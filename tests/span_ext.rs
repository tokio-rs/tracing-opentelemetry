use opentelemetry::trace::{Status, TracerProvider as _};
use opentelemetry_sdk::{
    error::OTelSdkResult,
    trace::{SdkTracerProvider, SpanData, SpanExporter, Tracer},
};
use std::sync::{Arc, Mutex};
use tracing::level_filters::LevelFilter;
use tracing::Subscriber;
use tracing_opentelemetry::{layer, OpenTelemetrySpanExt};
use tracing_subscriber::prelude::*;

#[derive(Clone, Default, Debug)]
struct TestExporter(Arc<Mutex<Vec<SpanData>>>);

impl SpanExporter for TestExporter {
    async fn export(&self, mut batch: Vec<SpanData>) -> OTelSdkResult {
        let spans = self.0.clone();
        if let Ok(mut inner) = spans.lock() {
            inner.append(&mut batch);
        }
        Ok(())
    }
}

fn test_tracer() -> (Tracer, SdkTracerProvider, TestExporter, impl Subscriber) {
    let exporter = TestExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let tracer = provider.tracer("test");

    let subscriber = tracing_subscriber::registry()
        .with(
            layer()
                .with_tracer(tracer.clone())
                .with_filter(LevelFilter::DEBUG),
        )
        .with(tracing_subscriber::fmt::layer().with_filter(LevelFilter::TRACE));

    (tracer, provider, exporter, subscriber)
}

#[test]
fn set_status_ok() {
    let root_span = set_status_helper(Status::Ok);
    assert_eq!(Status::Ok, root_span.status);
}

#[test]
fn set_status_error() {
    let expected_error = Status::Error {
        description: std::borrow::Cow::Borrowed("Elon put in too much fuel in his rocket!"),
    };
    let root_span = set_status_helper(expected_error.clone());
    assert_eq!(expected_error, root_span.status);
}

fn set_status_helper(status: Status) -> SpanData {
    let (_tracer, provider, exporter, subscriber) = test_tracer();

    tracing::subscriber::with_default(subscriber, || {
        let root = tracing::debug_span!("root").entered();

        root.set_status(status);
    });

    drop(provider); // flush all spans
    let spans = exporter.0.lock().unwrap();

    assert_eq!(spans.len(), 1);

    spans.iter().find(|s| s.name == "root").unwrap().clone()
}

#[test]
fn test_add_event() {
    let (_tracer, provider, exporter, subscriber) = test_tracer();

    let event_name = "my_event";
    let event_attrs = vec![
        opentelemetry::KeyValue::new("event_key_1", "event_value_1"),
        opentelemetry::KeyValue::new("event_key_2", 123),
    ];

    tracing::subscriber::with_default(subscriber, || {
        let root = tracing::debug_span!("root");
        let _enter = root.enter(); // Enter span to make it current for the event addition

        // Add the event using the new extension method
        root.add_event(event_name, event_attrs.clone());
    });

    drop(provider); // flush all spans
    let spans = exporter.0.lock().unwrap();

    assert_eq!(spans.len(), 1, "Should have exported exactly one span.");
    let root_span_data = spans.first().unwrap();

    assert_eq!(
        root_span_data.events.len(),
        1,
        "Span should have one event."
    );
    let event_data = root_span_data.events.first().unwrap();

    assert_eq!(event_data.name, event_name, "Event name mismatch.");
    assert_eq!(
        event_data.attributes, event_attrs,
        "Event attributes mismatch."
    );
}

#[test]
fn test_add_event_with_timestamp() {
    use std::time::{Duration, SystemTime};

    let (_tracer, provider, exporter, subscriber) = test_tracer();

    let event_name = "my_specific_time_event";
    let event_attrs = vec![opentelemetry::KeyValue::new("event_key_a", "value_a")];
    // Define a specific timestamp (e.g., 10 seconds ago)
    let specific_timestamp = SystemTime::now() - Duration::from_secs(10);

    tracing::subscriber::with_default(subscriber, || {
        let root = tracing::debug_span!("root_with_timestamped_event");
        let _enter = root.enter();

        // Add the event using the new extension method with the specific timestamp
        root.add_event_with_timestamp(event_name, specific_timestamp, event_attrs.clone());
    });

    drop(provider); // flush all spans
    let spans = exporter.0.lock().unwrap();

    assert_eq!(spans.len(), 1, "Should have exported exactly one span.");
    let root_span_data = spans.first().unwrap();

    assert_eq!(
        root_span_data.events.len(),
        1,
        "Span should have one event."
    );
    let event_data = root_span_data.events.first().unwrap();

    assert_eq!(event_data.name, event_name, "Event name mismatch.");
    assert_eq!(
        event_data.attributes, event_attrs,
        "Event attributes mismatch."
    );

    // Assert the timestamp matches the one we provided
    // Allow for a small tolerance due to potential precision differences during conversion
    let timestamp_diff = event_data
        .timestamp
        .duration_since(specific_timestamp)
        .unwrap_or_else(|_| {
            specific_timestamp
                .duration_since(event_data.timestamp)
                .unwrap_or_default()
        });
    assert!(
        timestamp_diff < Duration::from_millis(1),
        "Timestamp mismatch. Expected: {:?}, Got: {:?}",
        specific_timestamp,
        event_data.timestamp
    );
}
