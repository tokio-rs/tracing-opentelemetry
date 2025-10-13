use opentelemetry::global;
use opentelemetry::trace::TraceContextExt;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use std::collections::HashMap;
use tracing::{span, Span};
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Registry;

fn make_request() {
    let context = Span::current().context();

    assert!(context.span().span_context().is_valid());

    // Perform external request after injecting context. See `opentelemetry::propagation` for
    // details.
}

fn build_example_carrier() -> HashMap<String, String> {
    let mut carrier = HashMap::new();
    carrier.insert(
        "traceparent".to_string(),
        "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01".to_string(),
    );

    carrier
}

fn main() {
    // Set a format for propagating context. This MUST be provided, as the default is a no-op.
    global::set_text_map_propagator(TraceContextPropagator::new());
    let subscriber = Registry::default().with(tracing_opentelemetry::layer());

    tracing::subscriber::with_default(subscriber, || {
        // Extract context from request headers
        let parent_context = global::get_text_map_propagator(|propagator| {
            propagator.extract(&build_example_carrier())
        });

        // Generate tracing span as usual
        let app_root = span!(tracing::Level::INFO, "app_start");

        // Assign parent trace from external context
        if let Err(error) = app_root.set_parent(parent_context) {
            tracing::error!(
                error = debug(error),
                "Unable to set OpenTelemetry parent, span relationships will be wrong!"
            );
            // You don't want to panic in this case in production environment. Instead, you can log
            // this to know your traces may have wrong relationships but let the business logic
            // continue.
            panic!("Could not set parent.");
        }

        app_root.in_scope(|| {
            // The context can be accessed in the `tracing` span. Just make sure that the correct
            // `tracing` span is entered and the propagating library should be able to handle it.
            make_request();
        });
    });
}
