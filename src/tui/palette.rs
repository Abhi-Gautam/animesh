//! Command palette state. Nucleo integration lands in T40.

#[derive(Debug, Default)]
pub struct PaletteState {
    pub query: String,
    pub selected: usize,
}
