//! Option types shared by both tools.

use clap::ValueEnum;
use textra::correction::{DistanceCheckMode, ErrorCorrectionMode};
use textra::{ColoringStrategy, ErrorCorrectionConfig, FillRule};

/// Edge coloring strategy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Coloring {
    /// Switch color at every corner (msdfgen's default)
    Simple,
    /// Short splines between corners don't consume a color switch (default)
    Inktrap,
    /// Maximize the distance between edges of the same color
    Distance,
}

impl From<Coloring> for ColoringStrategy {
    fn from(c: Coloring) -> Self {
        match c {
            Coloring::Simple => ColoringStrategy::Simple,
            Coloring::Inktrap => ColoringStrategy::InkTrap,
            Coloring::Distance => ColoringStrategy::Distance,
        }
    }
}

/// MSDF error correction mode (msdfgen's names).
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum ErrorCorrection {
    /// No correction
    #[value(alias = "disable", alias = "none", alias = "0", alias = "false")]
    Disabled,
    /// Edge priority, exact distance check near edges (default)
    #[value(alias = "default", alias = "auto-mixed", alias = "mixed")]
    Auto,
    /// Edge priority, no distance check
    #[value(alias = "fast")]
    AutoFast,
    /// Edge priority, exact distance check everywhere
    #[value(alias = "full")]
    AutoFull,
    /// Correct everything, no distance check
    #[value(alias = "distance-fast", alias = "indiscriminate")]
    Distance,
    /// Correct everything, exact distance check
    #[value(alias = "indiscriminate-full")]
    DistanceFull,
    /// Only edges, no distance check
    EdgeFast,
    /// Only edges, exact distance check
    #[value(alias = "edge-full")]
    Edge,
}

impl ErrorCorrection {
    pub fn apply(self, ec: &mut ErrorCorrectionConfig) {
        use DistanceCheckMode as D;
        use ErrorCorrectionMode as M;
        let (m, d) = match self {
            ErrorCorrection::Disabled => (M::Disabled, D::DoNotCheck),
            ErrorCorrection::Auto => (M::EdgePriority, D::CheckAtEdge),
            ErrorCorrection::AutoFast => (M::EdgePriority, D::DoNotCheck),
            ErrorCorrection::AutoFull => (M::EdgePriority, D::AlwaysCheck),
            ErrorCorrection::Distance => (M::Indiscriminate, D::DoNotCheck),
            ErrorCorrection::DistanceFull => (M::Indiscriminate, D::AlwaysCheck),
            ErrorCorrection::EdgeFast => (M::EdgeOnly, D::DoNotCheck),
            ErrorCorrection::Edge => (M::EdgeOnly, D::AlwaysCheck),
        };
        ec.mode = m;
        ec.distance_check = d;
    }
}

/// Fill rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Fill {
    Nonzero,
    #[value(alias = "odd")]
    Evenodd,
    Positive,
    Negative,
}

impl From<Fill> for FillRule {
    fn from(f: Fill) -> Self {
        match f {
            Fill::Nonzero => FillRule::NonZero,
            Fill::Evenodd => FillRule::EvenOdd,
            Fill::Positive => FillRule::Positive,
            Fill::Negative => FillRule::Negative,
        }
    }
}

/// Parses an angle in radians, or degrees with a `D` suffix (`171.9D`).
pub fn parse_angle(s: &str) -> Result<f64, String> {
    let r = if let Some(d) = s.strip_suffix(['d', 'D']) {
        d.parse::<f64>().map(f64::to_radians)
    } else {
        s.parse::<f64>()
    };
    r.map_err(|_| format!("invalid angle '{s}' (radians, or degrees with a D suffix)"))
}

/// Parses `axis=value` for variable fonts.
pub fn parse_font_var(s: &str) -> Result<(String, f32), String> {
    let (k, v) = s
        .split_once('=')
        .ok_or_else(|| format!("expected axis=value, got '{s}'"))?;
    if k.len() != 4 {
        return Err(format!("variation axis tags have 4 characters, got '{k}'"));
    }
    let v = v.parse::<f32>().map_err(|_| format!("invalid value in '{s}'"))?;
    Ok((k.to_string(), v))
}

/// Parses an integer in decimal or `0x` hexadecimal.
pub fn parse_int(s: &str) -> Option<u32> {
    if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u32::from_str_radix(h, 16).ok()
    } else {
        s.parse().ok()
    }
}

/// Worker thread count (`0` = all cores).
pub fn thread_count(n: usize) -> usize {
    if n == 0 {
        std::thread::available_parallelism().map_or(1, std::num::NonZero::get)
    } else {
        n
    }
}
