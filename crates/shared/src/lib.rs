use opentelemetry::trace::TracerProvider;
use opentelemetry::{global, KeyValue};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_sdk::{
    metrics::{MeterProviderBuilder, PeriodicReader, SdkMeterProvider},
    trace::Sampler,
    Resource,
};
use opentelemetry_semantic_conventions::resource::{SERVICE_NAME, SERVICE_VERSION};
use opentelemetry_semantic_conventions::SCHEMA_URL;
use tracing_core::{Level, LevelFilter};
use tracing_opentelemetry::{MetricsLayer, OpenTelemetryLayer};
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{layer::SubscriberExt, Layer};

pub fn print_header(cargo_pkg_name: &str, cargo_pkg_version: &str, git_rev: Option<&str>) {
    println!(
        "rengarde-{} ver. {} rev. {}",
        cargo_pkg_name,
        cargo_pkg_version,
        git_rev.unwrap_or("UNKNOWN")
    );
}

pub struct OtelGuard {
    tracer_provider: Option<SdkTracerProvider>,
    meter_provider: Option<SdkMeterProvider>,
}

impl Drop for OtelGuard {
    fn drop(&mut self) {
        if let Some(tracer_provider) = &self.tracer_provider {
            if let Err(err) = tracer_provider.shutdown() {
                eprintln!("{err:?}");
            }
        }
        if let Some(meter_provider) = &self.meter_provider {
            if let Err(err) = meter_provider.shutdown() {
                eprintln!("{err:?}");
            }
        }
    }
}

pub fn init() -> OtelGuard {
    let endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok();
    init_tracing_subscriber(endpoint.as_deref())
}

fn resource() -> Resource {
    Resource::builder()
        .with_schema_url(
            [
                KeyValue::new(SERVICE_NAME, env!("CARGO_PKG_NAME")),
                KeyValue::new(SERVICE_VERSION, env!("CARGO_PKG_VERSION")),
            ],
            SCHEMA_URL,
        )
        .build()
}

fn init_meter_provider(endpoint: &str) -> SdkMeterProvider {
    let exporter = opentelemetry_otlp::MetricExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .build()
        .unwrap();

    let reader = PeriodicReader::builder(exporter)
        .with_interval(std::time::Duration::from_secs(5))
        .build();

    let meter_provider = MeterProviderBuilder::default()
        .with_resource(resource())
        .with_reader(reader)
        .build();

    global::set_meter_provider(meter_provider.clone());
    meter_provider
}

fn init_tracer_provider(endpoint: &str) -> SdkTracerProvider {
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .build()
        .unwrap();

    SdkTracerProvider::builder()
        .with_sampler(Sampler::AlwaysOn)
        .with_resource(resource())
        .with_batch_exporter(exporter)
        .build()
}

fn init_tracing_subscriber(endpoint: Option<&str>) -> OtelGuard {
    let tracer_provider = endpoint.map(init_tracer_provider);
    let meter_provider = endpoint.map(init_meter_provider);

    let tracer = tracer_provider
        .clone()
        .map(|t| t.tracer("tracing-otel-subscriber"));

    tracing_subscriber::registry()
        .with(LevelFilter::from_level(Level::DEBUG))
        .with(
            tracing_subscriber::fmt::layer()
                .with_level(true)
                .with_target(false)
                .with_thread_ids(true)
                .with_filter(
                    tracing_subscriber::EnvFilter::builder()
                        .with_default_directive(LevelFilter::INFO.into())
                        .from_env_lossy(),
                ),
        )
        .with(meter_provider.clone().map(MetricsLayer::new))
        .with(tracer.map(OpenTelemetryLayer::new))
        .init();

    OtelGuard {
        tracer_provider,
        meter_provider,
    }
}
