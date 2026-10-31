//! Registry data model: value tags, tuning surfaces and entries.

/// Kind-tagged threshold value used by the registry for equality checks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum ThresholdValue {
    U64(u64),
    U32(u32),
    Usize(usize),
    F64(f64),
    F32(f32),
    Bool(bool),
    Pair(u32, u32),
}

/// Surfaces a threshold can be retuned from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TunableLayers {
    /// `config.toml [storage]` key on `linkrs-config::StorageConfig`.
    pub(crate) config_file: bool,
    /// Field on an engine-side config struct.
    pub(crate) engine_struct: bool,
    /// Hardcoded constant only.
    pub(crate) hardcoded: bool,
}

pub(crate) const LAYER_AB: TunableLayers = TunableLayers {
    config_file: true,
    engine_struct: true,
    hardcoded: false,
};
pub(crate) const LAYER_B: TunableLayers = TunableLayers {
    config_file: false,
    engine_struct: true,
    hardcoded: false,
};
pub(crate) const LAYER_BC: TunableLayers = TunableLayers {
    config_file: false,
    engine_struct: true,
    hardcoded: true,
};
pub(crate) const LAYER_C: TunableLayers = TunableLayers {
    config_file: false,
    engine_struct: false,
    hardcoded: true,
};

/// One registered threshold.
pub(crate) struct ThresholdEntry {
    /// Identifier, usually the defining constant or config field name.
    pub(crate) id: &'static str,
    /// Value unit: `bytes`, `rows`, `ratio`, `percent`, `seconds`,
    /// `milliseconds`, `count`, `bits`, or `bool`.
    pub(crate) unit: &'static str,
    /// Defining file, relative to the crate root (or `linkrs-*` prefixed
    /// for cross-crate defaults).
    pub(crate) source: &'static str,
    /// Tuning surfaces.
    pub(crate) layers: TunableLayers,
    /// Returns the live default. Referencing the constant or config
    /// default here is the compile-time pin: renames break the build.
    pub(crate) live_default: fn() -> ThresholdValue,
    /// Documented default; asserted equal to `live_default()`.
    pub(crate) documented_default: ThresholdValue,
    /// Benchmark functions that observe this threshold.
    pub(crate) benches: &'static [&'static str],
    /// Modules whose behavior shifts when this threshold changes.
    pub(crate) affected_modules: &'static [&'static str],
    /// Free-form annotation; must explain the exemption when `benches`
    /// is empty.
    pub(crate) note: &'static str,
}
