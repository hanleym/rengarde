use anyhow::Result;
use opentelemetry::{global, KeyValue};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{metrics::{
    MeterProviderBuilder, PeriodicReader, SdkMeterProvider,
}, Resource, runtime, trace::{BatchConfig, RandomIdGenerator, Sampler, Tracer}};
use opentelemetry_sdk::metrics::reader::{DefaultAggregationSelector, DefaultTemporalitySelector};
use opentelemetry_semantic_conventions::resource::{DEPLOYMENT_ENVIRONMENT, SERVICE_NAME, SERVICE_VERSION};
use opentelemetry_semantic_conventions::SCHEMA_URL;
use tracing_core::{Level, LevelFilter};
use tracing_opentelemetry::{MetricsLayer, OpenTelemetryLayer};
use tracing_subscriber::{EnvFilter, Layer, layer::SubscriberExt, util::SubscriberInitExt};

pub struct Guard {
    meter_provider: Option<SdkMeterProvider>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Err(err) = self.meter_provider.as_ref().map_or(Ok(()), |m| m.shutdown()) {
            eprintln!("{err:?}");
        }
        global::shutdown_tracer_provider();
    }
}

pub fn init() -> Result<Guard> {
    let mut guard = Guard {
        meter_provider: None,
    };

    if let Ok(endpoint) = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT") {
        guard.meter_provider = Some(init_tracing_subscriber(&endpoint));
    }
    Ok(guard)
}

#[allow(clippy::too_many_arguments)]
pub fn print_header(
    rengarde_official_build: bool,
    cargo_pkg_name: &str,
    cargo_pkg_version: &str,
    vergen_git_describe: &str,
    vergen_git_dirty: &str,
    vergen_build_timestamp: &str,
    vergen_cargo_target_triple: &str,
    rust_runtime: &str,
) {
    let version_string = if rengarde_official_build {
        cargo_pkg_version
    } else {
        &format!(
            "{}{} built at {} for {} - UNOFFICIAL BUILD",
            vergen_git_describe,
            if vergen_git_dirty == "true" { "* (dirty)" } else { "" },
            vergen_build_timestamp.split_at(19).0,
            vergen_cargo_target_triple
        )
    };

    println!(
        "rengarde-{} ({}) ver. {}",
        cargo_pkg_name,
        rust_runtime,
        version_string,
    );
}

fn resource() -> Resource {
    Resource::from_schema_url(
        [
            KeyValue::new(SERVICE_NAME, env!("CARGO_PKG_NAME")),
            KeyValue::new(SERVICE_VERSION, env!("CARGO_PKG_VERSION")),
            // KeyValue::new(RUST_RUNTIME, env!("")),
            KeyValue::new(DEPLOYMENT_ENVIRONMENT, "develop"),
        ],
        SCHEMA_URL,
    )
}

fn init_meter_provider(endpoint: &str) -> SdkMeterProvider {
    let exporter = opentelemetry_otlp::new_exporter()
        .tonic()
        .with_endpoint(endpoint);

    let exporter = exporter
        .build_metrics_exporter(
            Box::new(DefaultAggregationSelector::new()),
            Box::new(DefaultTemporalitySelector::new()),
        )
        .unwrap();

    let reader = PeriodicReader::builder(exporter, runtime::Tokio)
        .with_interval(std::time::Duration::from_secs(5))
        .build();

    let meter_provider = MeterProviderBuilder::default()
        .with_resource(resource())
        .with_reader(reader)
        // .with_reader(stdout_reader)
        // .with_view(view_foo)
        // .with_view(view_baz)
        .build();

    global::set_meter_provider(meter_provider.clone());
    meter_provider
}

fn init_tracer(endpoint: &str) -> Tracer {
    let exporter = opentelemetry_otlp::new_exporter()
        .tonic()
        .with_endpoint(endpoint);

    opentelemetry_otlp::new_pipeline()
        .tracing()
        .with_trace_config(
            opentelemetry_sdk::trace::Config::default()
                // Customize sampling strategy
                .with_sampler(Sampler::ParentBased(Box::new(Sampler::TraceIdRatioBased(
                    1.0,
                ))))
                // If export trace to AWS X-Ray, you can use XrayIdGenerator
                .with_id_generator(RandomIdGenerator::default())
                .with_resource(resource()),
        )
        .with_batch_config(BatchConfig::default())
        .with_exporter(exporter)
        .install_batch(runtime::Tokio)
        .unwrap()
}

fn init_tracing_subscriber(endpoint: &str) -> SdkMeterProvider {
    let meter_provider = init_meter_provider(endpoint);

    tracing_subscriber::registry()
        .with(LevelFilter::from_level(
            Level::DEBUG,
        ))
        .with(tracing_subscriber::fmt::layer()
            .with_thread_ids(true)
            .with_target(false)
            .with_filter(
                EnvFilter::builder()
                    .with_default_directive(LevelFilter::from_level(Level::INFO).into())
                    .from_env_lossy(),
            )
        )
        .with(MetricsLayer::new(meter_provider.clone()))
        .with(OpenTelemetryLayer::new(init_tracer(endpoint)))
        .init();

    meter_provider
}
