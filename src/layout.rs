//! Speaker positions, for naming what each interleaved slot carries.

/// A speaker position. The names are the usual short ones (ffmpeg's among
/// others).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Speaker {
    /// Front left.
    FL,
    /// Front right.
    FR,
    /// Front centre (also a mono stream's one channel).
    FC,
    /// Low-frequency effects.
    LFE,
    /// Back left.
    BL,
    /// Back right.
    BR,
    /// Back centre.
    BC,
    /// Side left.
    SL,
    /// Side right.
    SR,
    /// Front left of centre (ALAC's eight-channel `Lc`).
    FLC,
    /// Front right of centre (ALAC's eight-channel `Rc`).
    FRC,
}
