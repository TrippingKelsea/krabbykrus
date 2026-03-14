//! OpenTelemetry integration for RockBot.
//!
//! Provides optional OTLP trace export when the `otel` feature is enabled.
//! Without the feature flag, `init_telemetry` logs that the facade-based
//! metrics (via `metrics` crate) remain available.

use serde::{Deserialize, Serialize};

/// Telemetry configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryConfig {
    /// Whether telemetry export is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// OTLP exporter endpoint (e.g. "http://localhost:4317").
    #[serde(default)]
    pub otlp_endpoint: Option<String>,
    /// Service name reported to the collector.
    #[serde(default = "default_service_name")]
    pub service_name: String,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            otlp_endpoint: None,
            service_name: default_service_name(),
        }
    }
}

fn default_service_name() -> String {
    "rockbot".to_string()
}

/// Initialize telemetry subsystem.
///
/// When the `otel` feature is enabled, this sets up an OTLP exporter for
/// distributed traces.  Without the feature flag, the existing
/// `tracing-subscriber` + `metrics` facade continue to work as-is.
pub fn init_telemetry(config: &TelemetryConfig) {
    if !config.enabled {
        tracing::debug!("Telemetry export disabled");
        return;
    }

    #[cfg(feature = "otel")]
    {
        init_otel(config);
    }

    #[cfg(not(feature = "otel"))]
    {
        let endpoint = config.otlp_endpoint.as_deref().unwrap_or("(not configured)");
        tracing::info!(
            "Telemetry enabled (endpoint={endpoint}). \
             Metrics available via GET /api/metrics. \
             OTLP export requires the 'otel' feature."
        );
    }
}

/// Set up the OpenTelemetry OTLP exporter and install a tracing layer.
#[cfg(feature = "otel")]
fn init_otel(config: &TelemetryConfig) {
    use opentelemetry::trace::TracerProvider as _;
    use opentelemetry_otlp::WithExportConfig;
    use opentelemetry_sdk::trace::TracerProvider;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let endpoint = config
        .otlp_endpoint
        .as_deref()
        .unwrap_or("http://localhost:4317");

    let exporter = match opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .build()
    {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("Failed to create OTLP exporter: {e}");
            return;
        }
    };

    let provider = TracerProvider::builder()
        .with_batch_exporter(exporter, opentelemetry_sdk::runtime::Tokio)
        .build();

    let tracer = provider.tracer(config.service_name.clone());

    let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);

    // Install the OTel layer alongside the existing env-filter subscriber
    if let Err(e) = tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .with(otel_layer)
        .try_init()
    {
        tracing::warn!("Could not install OTel tracing layer (subscriber already set?): {e}");
    }

    tracing::info!(
        "OpenTelemetry OTLP export enabled (endpoint={endpoint}, service={})",
        config.service_name
    );
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn test_default_config() {
        let config = TelemetryConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.service_name, "rockbot");
        assert!(config.otlp_endpoint.is_none());
    }

    #[test]
    fn test_init_disabled() {
        // Should be a no-op, not panic
        init_telemetry(&TelemetryConfig::default());
    }
}
