use opentelemetry::{
    metrics::MetricsError,
    sdk::{
        metrics::{
            data::{Histogram, ResourceMetrics, Sum},
            reader::{
                AggregationSelector, DefaultAggregationSelector, DefaultTemporalitySelector,
                MetricReader, TemporalitySelector,
            },
            InstrumentKind, ManualReader, MeterProvider,
        },
        Resource,
    },
    Context,
};
use std::{fmt::Debug, sync::Arc};
use tracing::Subscriber;
use tracing_opentelemetry::MetricsLayer;
use tracing_subscriber::prelude::*;

const CARGO_PKG_VERSION: &str = env!("CARGO_PKG_VERSION");
const INSTRUMENTATION_LIBRARY_NAME: &str = "tracing/tracing-opentelemetry";

#[tokio::test]
async fn u64_counter_is_exported() {
    let (subscriber, exporter) =
        init_subscriber("hello_world".to_string(), InstrumentKind::Counter, 1_u64);

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(monotonic_counter.hello_world = 1_u64);
    });

    exporter.export().unwrap();
}

#[tokio::test]
async fn u64_counter_is_exported_i64_at_instrumentation_point() {
    let (subscriber, exporter) =
        init_subscriber("hello_world2".to_string(), InstrumentKind::Counter, 1_u64);

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(monotonic_counter.hello_world2 = 1_i64);
    });

    exporter.export().unwrap();
}

#[tokio::test]
async fn f64_counter_is_exported() {
    let (subscriber, exporter) = init_subscriber(
        "float_hello_world".to_string(),
        InstrumentKind::Counter,
        1.000000123_f64,
    );

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(monotonic_counter.float_hello_world = 1.000000123_f64);
    });

    exporter.export().unwrap();
}

#[tokio::test]
async fn i64_up_down_counter_is_exported() {
    let (subscriber, exporter) =
        init_subscriber("pebcak".to_string(), InstrumentKind::UpDownCounter, -5_i64);

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(counter.pebcak = -5_i64);
    });

    exporter.export().unwrap();
}

#[tokio::test]
async fn i64_up_down_counter_is_exported_u64_at_instrumentation_point() {
    let (subscriber, exporter) =
        init_subscriber("pebcak2".to_string(), InstrumentKind::UpDownCounter, 5_i64);

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(counter.pebcak2 = 5_u64);
    });

    exporter.export().unwrap();
}

#[tokio::test]
async fn f64_up_down_counter_is_exported() {
    let (subscriber, exporter) = init_subscriber(
        "pebcak_blah".to_string(),
        InstrumentKind::UpDownCounter,
        99.123_f64,
    );

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(counter.pebcak_blah = 99.123_f64);
    });

    exporter.export().unwrap();
}

#[tokio::test]
async fn u64_histogram_is_exported() {
    let (subscriber, exporter) =
        init_subscriber("abcdefg".to_string(), InstrumentKind::Histogram, 9_u64);

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(histogram.abcdefg = 9_u64);
    });

    exporter.export().unwrap();
}

#[tokio::test]
async fn i64_histogram_is_exported() {
    let (subscriber, exporter) = init_subscriber(
        "abcdefg_auenatsou".to_string(),
        InstrumentKind::Histogram,
        -19_i64,
    );

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(histogram.abcdefg_auenatsou = -19_i64);
    });

    exporter.export().unwrap();
}

#[tokio::test]
async fn f64_histogram_is_exported() {
    let (subscriber, exporter) = init_subscriber(
        "abcdefg_racecar".to_string(),
        InstrumentKind::Histogram,
        777.0012_f64,
    );

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(histogram.abcdefg_racecar = 777.0012_f64);
    });

    exporter.export().unwrap();
}

fn init_subscriber<T>(
    expected_metric_name: String,
    expected_instrument_kind: InstrumentKind,
    expected_value: T,
) -> (impl Subscriber + 'static, TestExporter<T>) {
    let reader = ManualReader::builder()
        .with_aggregation_selector(Box::new(DefaultAggregationSelector::new()))
        .with_temporality_selector(DefaultTemporalitySelector::new())
        .build();
    let reader = TestReader {
        inner: Arc::new(reader),
    };

    let provider = MeterProvider::builder().with_reader(reader.clone()).build();
    let exporter = TestExporter {
        expected_metric_name,
        expected_instrument_kind,
        expected_value,
        reader,
        _meter_provider: provider.clone(),
    };

    (
        tracing_subscriber::registry().with(MetricsLayer::new(provider)),
        exporter,
    )
}

#[derive(Debug, Clone)]
struct TestReader {
    inner: Arc<ManualReader>,
}

impl AggregationSelector for TestReader {
    fn aggregation(&self, kind: InstrumentKind) -> opentelemetry::sdk::metrics::Aggregation {
        self.inner.aggregation(kind)
    }
}

impl TemporalitySelector for TestReader {
    fn temporality(&self, kind: InstrumentKind) -> opentelemetry::sdk::metrics::data::Temporality {
        self.inner.temporality(kind)
    }
}

impl MetricReader for TestReader {
    fn register_pipeline(&self, pipeline: std::sync::Weak<opentelemetry::sdk::metrics::Pipeline>) {
        self.inner.register_pipeline(pipeline);
    }

    fn register_producer(
        &self,
        producer: Box<dyn opentelemetry::sdk::metrics::reader::MetricProducer>,
    ) {
        self.inner.register_producer(producer);
    }

    fn collect(
        &self,
        rm: &mut opentelemetry::sdk::metrics::data::ResourceMetrics,
    ) -> opentelemetry::metrics::Result<()> {
        self.inner.collect(rm)
    }

    fn force_flush(&self, cx: &Context) -> opentelemetry::metrics::Result<()> {
        self.inner.force_flush(cx)
    }

    fn shutdown(&self) -> opentelemetry::metrics::Result<()> {
        self.inner.shutdown()
    }
}

struct TestExporter<T> {
    expected_metric_name: String,
    expected_instrument_kind: InstrumentKind,
    expected_value: T,
    reader: TestReader,
    _meter_provider: MeterProvider,
}

impl<T> TestExporter<T>
where
    T: Debug + PartialEq + Copy + std::iter::Sum + 'static,
{
    fn export(&self) -> Result<(), MetricsError> {
        let mut rm = ResourceMetrics {
            resource: Resource::default(),
            scope_metrics: Vec::new(),
        };
        self.reader.collect(&mut rm)?;

        assert!(!rm.scope_metrics.is_empty());

        rm.scope_metrics.into_iter().for_each(|scope_metrics| {
            assert_eq!(scope_metrics.scope.name, INSTRUMENTATION_LIBRARY_NAME);
            assert_eq!(
                scope_metrics.scope.version.unwrap().as_ref(),
                CARGO_PKG_VERSION
            );

            scope_metrics.metrics.into_iter().for_each(|metric| {
                assert_eq!(metric.name, self.expected_metric_name);

                match self.expected_instrument_kind {
                    InstrumentKind::Counter | InstrumentKind::UpDownCounter => {
                        let sum = metric.data.as_any().downcast_ref::<Sum<T>>().unwrap();
                        assert_eq!(
                            self.expected_value,
                            sum.data_points
                                .iter()
                                .map(|data_point| data_point.value)
                                .sum()
                        );
                    }
                    InstrumentKind::Histogram => {
                        let histogram =
                            metric.data.as_any().downcast_ref::<Histogram<T>>().unwrap();
                        let histogram_data = histogram.data_points.first().unwrap();
                        assert!(histogram_data.count > 0);
                        assert_eq!(histogram_data.sum, self.expected_value);
                    }
                    unexpected => {
                        panic!("InstrumentKind {:?} not currently supported!", unexpected)
                    }
                }
            });
        });

        Ok(())
    }
}
