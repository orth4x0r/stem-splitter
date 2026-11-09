//! The minimal interface the separators need from an inference backend.
//!
//! Separators talk to a `dyn Network` rather than to `ort` directly so their chunking / STFT
//! logic can be unit-tested with plain closures, without an ONNX Runtime library.

use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use crate::{Device, ModelError, Result};

/// One `f32` input tensor (row-major).
pub(crate) struct TensorIn<'a> {
    pub shape: Vec<usize>,
    pub data: &'a [f32],
}

/// One `f32` output tensor (row-major).
#[derive(Debug, Clone)]
pub(crate) struct TensorOut {
    pub shape: Vec<usize>,
    pub data: Vec<f32>,
}

pub(crate) trait Network: Send {
    /// Declared input shapes in graph order; `-1` marks a dynamic dimension.
    fn input_shapes(&self) -> Vec<Vec<i64>>;
    /// Input names in graph order (empty when the backend has none, e.g. test stubs).
    fn input_names(&self) -> Vec<String> {
        Vec::new()
    }
    /// Output names in graph order (empty when the backend has none).
    fn output_names(&self) -> Vec<String> {
        Vec::new()
    }
    /// Run with inputs in graph order; outputs come back in graph order.
    fn run(&mut self, inputs: &[TensorIn<'_>]) -> Result<Vec<TensorOut>>;
    /// Where it runs.
    fn device(&self) -> Device;
}

/// An ONNX Runtime session.
pub(crate) struct OrtNetwork {
    pub session: ort::session::Session,
    pub device: Device,
}

fn ort_err(e: impl std::fmt::Display) -> ModelError {
    ModelError::Runtime(e.to_string())
}

impl Network for OrtNetwork {
    fn input_shapes(&self) -> Vec<Vec<i64>> {
        self.session
            .inputs()
            .iter()
            .map(|o| match o.dtype() {
                ort::value::ValueType::Tensor { shape, .. } => shape.to_vec(),
                _ => Vec::new(),
            })
            .collect()
    }

    fn input_names(&self) -> Vec<String> {
        self.session
            .inputs()
            .iter()
            .map(|o| o.name().to_string())
            .collect()
    }

    fn output_names(&self) -> Vec<String> {
        self.session
            .outputs()
            .iter()
            .map(|o| o.name().to_string())
            .collect()
    }

    fn run(&mut self, inputs: &[TensorIn<'_>]) -> Result<Vec<TensorOut>> {
        let names: Vec<String> = self
            .session
            .inputs()
            .iter()
            .map(|o| o.name().to_string())
            .collect();
        if names.len() != inputs.len() {
            return Err(ModelError::Runtime(format!(
                "model expects {} inputs, got {}",
                names.len(),
                inputs.len()
            )));
        }
        let mut feed: Vec<(Cow<'_, str>, ort::session::SessionInputValue<'_>)> =
            Vec::with_capacity(inputs.len());
        for (name, t) in names.iter().zip(inputs) {
            let tensor = ort::value::TensorRef::from_array_view((t.shape.clone(), t.data))
                .map_err(ort_err)?;
            feed.push((Cow::Borrowed(name.as_str()), tensor.into()));
        }
        let outputs = self.session.run(feed).map_err(ort_err)?;
        let mut out = Vec::with_capacity(outputs.len());
        for (_, value) in outputs.iter() {
            let (shape, data) = value.try_extract_tensor::<f32>().map_err(ort_err)?;
            out.push(TensorOut {
                shape: shape.iter().map(|&d| d.max(0) as usize).collect(),
                data: data.to_vec(),
            });
        }
        Ok(out)
    }

    fn device(&self) -> Device {
        self.device
    }
}

/// One input tensor that owns its data, so it can be handed to the inference thread.
#[derive(Debug, Clone, Default)]
pub(crate) struct OwnedInput {
    pub shape: Vec<usize>,
    pub data: Vec<f32>,
}

impl OwnedInput {
    /// Reuse this buffer for a tensor of `shape`: resized to fit, contents unspecified.
    pub fn reshape(&mut self, shape: Vec<usize>) -> &mut [f32] {
        self.data.resize(shape.iter().product(), 0.0);
        self.shape = shape;
        &mut self.data
    }
}

fn run_owned(net: &mut dyn Network, inputs: &[OwnedInput]) -> Result<Vec<TensorOut>> {
    let views: Vec<TensorIn<'_>> = inputs
        .iter()
        .map(|t| TensorIn {
            shape: t.shape.clone(),
            data: &t.data,
        })
        .collect();
    net.run(&views)
}

/// Run `jobs` network calls in order: `prepare(i, inputs)` fills job `i`'s inputs (the slice
/// holds recycled buffers, see [`OwnedInput::reshape`]; resize it to the input count) and
/// `finish(i, outputs)` takes its outputs. Both run on the calling thread.
///
/// With `overlap`, the network runs on its own thread, so `prepare(i + 1)` and `finish(i - 1)`
/// (the STFTs, overlap-add) happen while the GPU works on job `i` instead of in between. Leave
/// it off for CPU inference, where onnxruntime already uses every core.
pub(crate) fn run_pipelined(
    net: &mut dyn Network,
    jobs: usize,
    overlap: bool,
    cancel: &AtomicBool,
    mut prepare: impl FnMut(usize, &mut Vec<OwnedInput>) -> Result<()>,
    mut finish: impl FnMut(usize, Vec<TensorOut>) -> Result<()>,
) -> Result<()> {
    let cancelled = || cancel.load(Ordering::Relaxed);
    if !overlap || jobs < 2 {
        let mut inputs = Vec::new();
        for i in 0..jobs {
            if cancelled() {
                return Err(ModelError::Cancelled);
            }
            prepare(i, &mut inputs)?;
            let out = run_owned(net, &inputs)?;
            finish(i, out)?;
        }
        return Ok(());
    }
    if cancelled() {
        return Err(ModelError::Cancelled);
    }
    let gone = || ModelError::Runtime("the inference thread stopped".into());
    std::thread::scope(|s| {
        // Capacity 1 each way: at most one job queued behind the one running.
        let (in_tx, in_rx) = mpsc::sync_channel::<Vec<OwnedInput>>(1);
        let (out_tx, out_rx) = mpsc::sync_channel::<(Vec<OwnedInput>, Result<Vec<TensorOut>>)>(1);
        std::thread::Builder::new()
            .name("inference".into())
            .spawn_scoped(s, move || {
                for inputs in in_rx {
                    let res = run_owned(net, &inputs);
                    let failed = res.is_err();
                    if out_tx.send((inputs, res)).is_err() || failed {
                        break;
                    }
                }
            })
            .map_err(|e| ModelError::Runtime(format!("starting the inference thread: {e}")))?;

        let mut spare: Vec<Vec<OwnedInput>> = Vec::new();
        let mut submit = |i: usize, spare: &mut Vec<Vec<OwnedInput>>| -> Result<()> {
            let mut inputs = spare.pop().unwrap_or_default();
            prepare(i, &mut inputs)?;
            in_tx.send(inputs).map_err(|_| gone())
        };
        let mut drive = || -> Result<()> {
            submit(0, &mut spare)?;
            for i in 0..jobs {
                if i + 1 < jobs {
                    if cancelled() {
                        return Err(ModelError::Cancelled);
                    }
                    if let Err(e) = submit(i + 1, &mut spare) {
                        // A failed run stops the thread; report its error, not the send's.
                        if let Ok((_, Err(run_err))) = out_rx.recv() {
                            return Err(run_err);
                        }
                        return Err(e);
                    }
                }
                let (inputs, res) = out_rx.recv().map_err(|_| gone())?;
                spare.push(inputs);
                finish(i, res?)?;
            }
            Ok(())
        };
        // Returning drops both channel ends before the scope joins the thread, so it exits
        // after at most the run in flight.
        drive()
    })
}

/// A closure-backed network for tests.
#[cfg(test)]
pub(crate) struct FnNetwork<F> {
    pub shapes: Vec<Vec<i64>>,
    pub f: F,
}

#[cfg(test)]
impl<F> Network for FnNetwork<F>
where
    F: FnMut(&[TensorIn<'_>]) -> Vec<TensorOut> + Send,
{
    fn input_shapes(&self) -> Vec<Vec<i64>> {
        self.shapes.clone()
    }
    fn run(&mut self, inputs: &[TensorIn<'_>]) -> Result<Vec<TensorOut>> {
        Ok((self.f)(inputs))
    }
    fn device(&self) -> Device {
        Device::Cpu
    }
}

/// Wraps a test network so it reports a GPU, which turns on the overlapped path.
#[cfg(test)]
pub(crate) struct OnGpu<N>(pub N);

#[cfg(test)]
impl<N: Network> Network for OnGpu<N> {
    fn input_shapes(&self) -> Vec<Vec<i64>> {
        self.0.input_shapes()
    }
    fn run(&mut self, inputs: &[TensorIn<'_>]) -> Result<Vec<TensorOut>> {
        self.0.run(inputs)
    }
    fn device(&self) -> Device {
        Device::DirectMl(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Doubles its one input; fails on the call numbered `fail_at`.
    struct Doubler {
        calls: usize,
        fail_at: Option<usize>,
    }

    impl Network for Doubler {
        fn input_shapes(&self) -> Vec<Vec<i64>> {
            vec![vec![-1]]
        }
        fn run(&mut self, inputs: &[TensorIn<'_>]) -> Result<Vec<TensorOut>> {
            let call = self.calls;
            self.calls += 1;
            if Some(call) == self.fail_at {
                return Err(ModelError::Runtime(format!("boom at {call}")));
            }
            Ok(vec![TensorOut {
                shape: inputs[0].shape.clone(),
                data: inputs[0].data.iter().map(|v| v * 2.0).collect(),
            }])
        }
        fn device(&self) -> Device {
            Device::Cpu
        }
    }

    fn drive(overlap: bool, jobs: usize, fail_at: Option<usize>) -> (Result<()>, Vec<f32>) {
        let mut net = Doubler { calls: 0, fail_at };
        let mut got = Vec::new();
        let res = run_pipelined(
            &mut net,
            jobs,
            overlap,
            &AtomicBool::new(false),
            |i, inputs| {
                inputs.resize_with(1, OwnedInput::default);
                // Recycled buffers come back with old contents: overwrite all of it.
                inputs[0]
                    .reshape(vec![2])
                    .copy_from_slice(&[i as f32, -(i as f32)]);
                Ok(())
            },
            |_, out| {
                got.extend_from_slice(&out[0].data);
                Ok(())
            },
        );
        (res, got)
    }

    #[test]
    fn overlapped_matches_serial() {
        for jobs in [0, 1, 2, 7] {
            let (a, x) = drive(false, jobs, None);
            let (b, y) = drive(true, jobs, None);
            assert!(a.is_ok() && b.is_ok());
            assert_eq!(x, y);
            assert_eq!(x.len(), 2 * jobs);
        }
    }

    #[test]
    fn overlapped_reports_the_run_error() {
        for fail_at in [0, 3, 6] {
            let (res, got) = drive(true, 7, Some(fail_at));
            match res {
                Err(ModelError::Runtime(m)) => assert_eq!(m, format!("boom at {fail_at}")),
                other => panic!("{other:?}"),
            }
            assert_eq!(got.len(), 2 * fail_at, "jobs before the failure finished");
        }
    }

    #[test]
    fn overlapped_honours_cancel() {
        let cancel = AtomicBool::new(false);
        let mut net = Doubler {
            calls: 0,
            fail_at: None,
        };
        let mut finished = 0;
        let res = run_pipelined(
            &mut net,
            10,
            true,
            &cancel,
            |_, inputs| {
                inputs.resize_with(1, OwnedInput::default);
                inputs[0].reshape(vec![1]);
                Ok(())
            },
            |i, _| {
                finished += 1;
                if i == 2 {
                    cancel.store(true, Ordering::Relaxed);
                }
                Ok(())
            },
        );
        assert!(matches!(res, Err(ModelError::Cancelled)));
        assert_eq!(finished, 3);
    }
}
