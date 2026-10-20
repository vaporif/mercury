use eyre::Context;
use opentelemetry::global;
use opentelemetry::metrics::MeterProvider;
use opentelemetry::trace::TracerProvider;
use opentelemetry_otlp::{Protocol, WithExportConfig};
use opentelemetry_sdk::metrics::{Aggregation, Instrument, SdkMeterProvider, Stream};
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_system_metrics::init_process_observer_once;
use serde::Deserialize;
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::registry::LookupSpan;

pub mod guard;
pub mod metric;
pub mod recorder;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TelemetryConfig {
    #[serde(default)]
    pub otlp_endpoint: Option<String>,
}

pub struct TelemetryGuard {
    meter_provider: Option<SdkMeterProvider>,
    tracer_provider: Option<SdkTracerProvider>,
}

impl TelemetryGuard {
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        self.meter_provider.is_some()
    }

    #[must_use]
    pub fn otel_layer<S>(&self) -> Option<OpenTelemetryLayer<S, opentelemetry_sdk::trace::Tracer>>
    where
        S: tracing::Subscriber + for<'span> LookupSpan<'span>,
    {
        self.tracer_provider.as_ref().map(|tp| {
            let tracer = tp.tracer("mercury");
            tracing_opentelemetry::layer().with_tracer(tracer)
        })
    }
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        if let Some(ref mp) = self.meter_provider {
            let _ = mp.shutdown();
        }
        if let Some(ref tp) = self.tracer_provider {
            let _ = tp.shutdown();
        }
    }
}

pub fn init(config: &TelemetryConfig) -> eyre::Result<TelemetryGuard> {
    let Some(ref endpoint) = config.otlp_endpoint else {
        return Ok(TelemetryGuard {
            meter_provider: None,
            tracer_provider: None,
        });
    };

    let metric_exporter = opentelemetry_otlp::MetricExporter::builder()
        .with_http()
        .with_protocol(Protocol::HttpBinary)
        .with_endpoint(endpoint)
        .build()
        .wrap_err("failed to build OTLP metric exporter")?;

    let meter_provider = SdkMeterProvider::builder()
        .with_periodic_exporter(metric_exporter)
        .with_view(histogram_view)
        .build();

    global::set_meter_provider(meter_provider.clone());

    let sys_meter = meter_provider.meter("mercury_system");
    tokio::spawn(async move {
        if let Err(e) = init_process_observer_once(sys_meter).await {
            tracing::warn!("failed to init system metrics observer: {e}");
        }
    });

    let span_exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_protocol(Protocol::HttpBinary)
        .with_endpoint(endpoint)
        .build()
        .wrap_err("failed to build OTLP span exporter")?;

    let tracer_provider = SdkTracerProvider::builder()
        .with_batch_exporter(span_exporter)
        .build();

    Ok(TelemetryGuard {
        meter_provider: Some(meter_provider),
        tracer_provider: Some(tracer_provider),
    })
}

fn histogram_view(instrument: &Instrument) -> Option<Stream> {
    let name = instrument.name();
    let buckets = if name.ends_with("latency_submitted_ms") {
        &metric::tx::TX_LATENCY_SUBMITTED_BUCKETS[..]
    } else if name.ends_with("latency_confirmed_ms") {
        &metric::tx::TX_LATENCY_CONFIRMED_BUCKETS[..]
    } else if name.ends_with("gas_paid") {
        &metric::tx::GAS_PAID_BUCKETS[..]
    } else if name.ends_with("query_latency_ms") {
        &metric::query::QUERY_LATENCY_BUCKETS[..]
    } else if name.ends_with("gas_price_gwei") {
        &metric::gas::GAS_PRICE_GWEI_BUCKETS[..]
    } else {
        return None;
    };

    Stream::builder()
        .with_aggregation(Aggregation::ExplicitBucketHistogram {
            boundaries: buckets.to_vec(),
            record_min_max: true,
        })
        .build()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_no_endpoint() {
        let config = TelemetryConfig::default();
        assert!(config.otlp_endpoint.is_none());
    }

    #[test]
    fn init_noop_when_disabled() {
        let config = TelemetryConfig::default();
        let guard = init(&config).unwrap();
        assert!(!guard.is_enabled());
    }

    #[test]
    fn deserialize_from_toml() {
        let toml_str = r#"
            otlp_endpoint = "http://localhost:4318"
        "#;
        let config: TelemetryConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(
            config.otlp_endpoint.as_deref(),
            Some("http://localhost:4318")
        );
    }

    #[test]
    fn deserialize_minimal_toml() {
        let toml_str = "";
        let config: TelemetryConfig = toml::from_str(toml_str).unwrap();
        assert!(config.otlp_endpoint.is_none());
    }

    #[test]
    fn deserialize_legacy_fields_ignored() {
        let toml_str = r#"
            metrics_port = 9090
            metrics_host = "0.0.0.0"
            otlp_endpoint = "http://localhost:4318"
        "#;
        let config: TelemetryConfig = toml::from_str(toml_str).unwrap();
        assert!(config.otlp_endpoint.is_some());
    }

    #[test]
    fn disabled_guard_has_no_otel_layer() {
        let config = TelemetryConfig::default();
        let guard = init(&config).unwrap();
        let layer: Option<
            tracing_opentelemetry::OpenTelemetryLayer<
                tracing_subscriber::Registry,
                opentelemetry_sdk::trace::Tracer,
            >,
        > = guard.otel_layer();
        assert!(layer.is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn init_with_endpoint_enables_metrics_and_tracing() {
        let config = TelemetryConfig {
            // Exporters connect lazily, so an unreachable endpoint still builds.
            otlp_endpoint: Some("http://127.0.0.1:9".to_string()),
        };
        let (harness_provider, _) = crate::recorder::tests::harness();
        let guard = init(&config).unwrap();
        // init() replaces the global meter provider; hand it back so recorder
        // tests sharing this process under `cargo test` keep their exporter.
        global::set_meter_provider(harness_provider.clone());
        assert!(guard.is_enabled());
        let layer: Option<
            tracing_opentelemetry::OpenTelemetryLayer<
                tracing_subscriber::Registry,
                opentelemetry_sdk::trace::Tracer,
            >,
        > = guard.otel_layer();
        assert!(layer.is_some());
        drop(guard);
    }

    fn boundaries_for(instrument: &str) -> Vec<f64> {
        use opentelemetry_sdk::metrics::data::{
            AggregatedMetrics, MetricData, ResourceMetrics, ScopeMetrics,
        };
        use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};

        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_periodic_exporter(exporter.clone())
            .with_view(histogram_view)
            .build();
        provider
            .meter("view-test")
            .f64_histogram(instrument.to_string())
            .build()
            .record(1.0, &[]);
        provider.force_flush().unwrap();

        let exports = exporter.get_finished_metrics().unwrap();
        let metric = exports
            .iter()
            .flat_map(ResourceMetrics::scope_metrics)
            .flat_map(ScopeMetrics::metrics)
            .find(|m| m.name() == instrument)
            .unwrap();
        let AggregatedMetrics::F64(MetricData::Histogram(hist)) = metric.data() else {
            panic!("{instrument} is not an f64 histogram");
        };
        let point = hist.data_points().next().unwrap();
        point.bounds().collect()
    }

    #[test]
    fn histogram_view_applies_metric_specific_buckets() {
        let cases: [(&str, &[f64]); 5] = [
            (
                metric::tx::TX_LATENCY_SUBMITTED_MS,
                &metric::tx::TX_LATENCY_SUBMITTED_BUCKETS,
            ),
            (
                metric::tx::TX_LATENCY_CONFIRMED_MS,
                &metric::tx::TX_LATENCY_CONFIRMED_BUCKETS,
            ),
            (metric::tx::GAS_PAID, &metric::tx::GAS_PAID_BUCKETS),
            (
                metric::query::QUERY_LATENCY_MS,
                &metric::query::QUERY_LATENCY_BUCKETS,
            ),
            (
                metric::gas::GAS_PRICE_GWEI,
                &metric::gas::GAS_PRICE_GWEI_BUCKETS,
            ),
        ];
        for (name, buckets) in cases {
            assert_eq!(boundaries_for(name), buckets, "{name}");
            // Matching is by suffix, so prefixed names get the same buckets.
            let prefixed = format!("mercury_{name}");
            assert_eq!(boundaries_for(&prefixed), buckets, "{prefixed}");
        }
    }

    #[test]
    fn histogram_view_leaves_other_histograms_on_sdk_defaults() {
        let sdk_default = [
            0.0, 5.0, 10.0, 25.0, 50.0, 75.0, 100.0, 250.0, 500.0, 750.0, 1000.0, 2500.0, 5000.0,
            7500.0, 10000.0,
        ];
        assert_eq!(boundaries_for("sweep_scan_duration_seconds"), sdk_default);
    }
}
