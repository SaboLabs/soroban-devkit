use std::str::FromStr;

/// Defines the output formatting style for CLI and RPC responses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    /// Compact JSON format.
    #[default]
    Json,
    /// Human-readable, pretty-printed JSON.
    Pretty,
}

impl FromStr for OutputFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "json" => Ok(OutputFormat::Json),
            "pretty" => Ok(OutputFormat::Pretty),
            _ => Err(format!(
                "Invalid format '{}'. Expected 'json' or 'pretty'.",
                s
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_output_format_from_str() {
        assert_eq!("json".parse::<OutputFormat>(), Ok(OutputFormat::Json));
        assert_eq!("JSON".parse::<OutputFormat>(), Ok(OutputFormat::Json));
        assert_eq!("pretty".parse::<OutputFormat>(), Ok(OutputFormat::Pretty));
        assert!("invalid".parse::<OutputFormat>().is_err());
    }
}
