use std::fmt;
use std::str::FromStr;

/// Output format for CLI display.
///
/// Canonical type lives in `sdkt-core` so all crates
/// (`sdkt-xdr`, `sdkt-rpc`, `sdkt-cli`) share one definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    /// Compact single-line JSON.
    Json,
    /// Pretty-printed JSON (default).
    #[default]
    Pretty,
}

impl fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OutputFormat::Json => write!(f, "json"),
            OutputFormat::Pretty => write!(f, "pretty"),
        }
    }
}

impl FromStr for OutputFormat {
    type Err = OutputFormatParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "json" => Ok(OutputFormat::Json),
            "pretty" => Ok(OutputFormat::Pretty),
            other => Err(OutputFormatParseError(other.to_string())),
        }
    }
}

/// Error returned when parsing an unrecognized output format string.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown output format: {0}; expected \"json\" or \"pretty\"")]
pub struct OutputFormatParseError(pub String);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_is_pretty() {
        assert_eq!(OutputFormat::default(), OutputFormat::Pretty);
    }

    #[test]
    fn test_from_str_json() {
        assert_eq!("json".parse::<OutputFormat>().unwrap(), OutputFormat::Json);
    }

    #[test]
    fn test_from_str_pretty() {
        assert_eq!("pretty".parse::<OutputFormat>().unwrap(), OutputFormat::Pretty);
    }

    #[test]
    fn test_from_str_case_insensitive() {
        assert_eq!("JSON".parse::<OutputFormat>().unwrap(), OutputFormat::Json);
        assert_eq!("Pretty".parse::<OutputFormat>().unwrap(), OutputFormat::Pretty);
    }

    #[test]
    fn test_from_str_unknown() {
        let err = "xml".parse::<OutputFormat>().unwrap_err();
        assert!(err.to_string().contains("unknown output format"));
    }

    #[test]
    fn test_display_roundtrip() {
        for fmt in [OutputFormat::Json, OutputFormat::Pretty] {
            let s = fmt.to_string();
            let parsed: OutputFormat = s.parse().unwrap();
            assert_eq!(parsed, fmt);
        }
    }
}
