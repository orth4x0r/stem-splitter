//! The minimal interface the separators need from an inference backend.
//!
//! Separators talk to a `dyn Network` rather than to `ort` directly so their chunking / STFT
//! logic can be unit-tested with plain closures, without an ONNX Runtime library.

use std::borrow::Cow;

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
