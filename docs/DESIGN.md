# Visual direction

Set in the "unslop" pass (Nov 2026), using the checklist from
[heydamir/unslop](https://github.com/heydamir/unslop) (CC BY 4.0, by Damir Sokolovsky) as a review
guide. No code or text from it is included in the app.

- **Product.** Pulls a track apart into stems for samples, beats and remixes.
- **Character.** Focused, tactile, a little warm.
- **World.** A mixing desk in a dark room: graphite surfaces, coloured channel strips, a lit VU lamp.
- **Moments.** The waveforms and the per-stem channel colours. That is where the colour goes.
- **Signature.** The stem's channel colour tab, used for stems and nothing else.

Rules for new UI, with tokens in `crates/stem-app/src/theme.rs`:

- Chrome is the graphite ramp (`BG`, `BG_PANEL`, `CARD`, `CARD_HI`, `STROKE`) and three text roles.
- One accent, `ACCENT` (amber), for the playhead, the selection, progress and the one key figure.
  The primary button is off-white (`PRIMARY`) with dark text.
- No coloured glows, gradients, gradient text, colour washes or sheens. `paint::glow` only draws
  neutral black shadows. Depth comes from surface steps and hairlines.
- Labels are sentence case (`widgets::sentence_case`), with no tracked uppercase. Headings use Inter
  Medium; hierarchy comes from size and colour.
- Motion explains something (progress, playback, a finished split). Hover is a surface change,
  not movement. Springs don't overshoot.
