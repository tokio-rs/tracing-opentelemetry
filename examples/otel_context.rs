//! This example demonstrates how to use `OpenTelemetryContext` from a separate layer
//! to access OpenTelemetry context data from tracing spans.

use opentelemetry::trace::{TraceContextExt, TracerProvider as _};
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_stdout as stdout;
use std::sync::{Arc, Mutex, OnceLock};
use tracing::{debug, info, span, warn, Subscriber};
use tracing::{dispatcher::WeakDispatch, level_filters::LevelFilter, Dispatch};
use tracing_opentelemetry::{get_otel_context, layer};
use tracing_subscriber::layer::Context;
use tracing_subscriber::prelude::*;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::Layer;

/// A custom layer that demonstrates how to use OpenTelemetryContext
/// to extract OpenTelemetry contexts from span extensions.
#[derive(Clone, Default)]
struct SpanAnalysisLayer {
    /// Store span analysis results for demonstration
    analysis_results: Arc<Mutex<Vec<SpanAnalysis>>>,
    /// Weak reference to the dispatcher for context extraction
    dispatch: Arc<OnceLock<WeakDispatch>>,
}

#[derive(Debug, Clone)]
struct SpanAnalysis {
    span_name: String,
    trace_id: String,
    span_id: String,
    is_sampled: bool,
}

impl SpanAnalysisLayer {
    fn get_analysis_results(&self) -> Vec<SpanAnalysis> {
        self.analysis_results.lock().unwrap().clone()
    }

    fn analyze_span_context(&self, span_name: &str, otel_context: &opentelemetry::Context) {
        let span = otel_context.span();
        let span_context = span.span_context();

        if span_context.is_valid() {
            let analysis = SpanAnalysis {
                span_name: span_name.to_string(),
                trace_id: format!("{:032x}", span_context.trace_id()),
                span_id: format!("{:016x}", span_context.span_id()),
                is_sampled: span_context.is_sampled(),
            };

            println!(
                "🔍 Analyzing span '{}': trace_id={}, span_id={}, sampled={}",
                analysis.span_name,
                analysis.trace_id,
                analysis.span_id,
                span_context.trace_flags().is_sampled()
            );

            if let Ok(mut results) = self.analysis_results.lock() {
                results.push(analysis);
            }
        }
    }
}

impl<S> Layer<S> for SpanAnalysisLayer
where
    S: Subscriber + for<'span> LookupSpan<'span>,
{
    fn on_register_dispatch(&self, subscriber: &Dispatch) {
        let _ = self.dispatch.set(subscriber.downgrade());
    }

    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        id: &tracing::span::Id,
        ctx: Context<'_, S>,
    ) {
        let Some(weak_dispatch) = self.dispatch.get() else {
            return;
        };

        // Get the span reference and extract OpenTelemetry context
        if let Some(span_ref) = ctx.span(id) {
            // This is the key functionality: using OpenTelemetryContext
            // to extract the OpenTelemetry context from span extensions
            let mut extensions = span_ref.extensions_mut();
            if let Some(dispatch) = weak_dispatch.upgrade() {
                if let Some(otel_context) = get_otel_context(&mut extensions, &dispatch) {
                    self.analyze_span_context(attrs.metadata().name(), &otel_context);
                } else {
                    println!(
                        "⚠️  Could not extract OpenTelemetry context for span '{}'",
                        attrs.metadata().name()
                    );
                }
            }
        }
    }

    fn on_enter(&self, id: &tracing::span::Id, ctx: Context<'_, S>) {
        if let Some(weak_dispatch) = self.dispatch.get() {
            if let Some(span_ref) = ctx.span(id) {
                let mut extensions = span_ref.extensions_mut();
                if let Some(dispatch) = weak_dispatch.upgrade() {
                    if let Some(otel_context) = get_otel_context(&mut extensions, &dispatch) {
                        let span = otel_context.span();
                        let span_context = span.span_context();
                        if span_context.is_valid() {
                            println!(
                                "📍 Entering span with trace_id: {:032x}, span_id: {:016x}",
                                span_context.trace_id(),
                                span_context.span_id()
                            );
                        }
                    }
                }
            }
        }
    }
}

fn setup_tracing() -> (impl Subscriber, SdkTracerProvider, SpanAnalysisLayer) {
    // Create OpenTelemetry tracer that outputs to stdout
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(stdout::SpanExporter::default())
        .build();
    let tracer = provider.tracer("span_ref_ext_example");

    // Create our custom analysis layer
    let analysis_layer = SpanAnalysisLayer::default();

    // Build the subscriber with multiple layers:
    // 1. OpenTelemetry layer for trace export
    // 2. Our custom analysis layer that uses OpenTelemetryContext
    // 3. Formatting layer for console output
    let subscriber = tracing_subscriber::registry()
        .with(layer().with_tracer(tracer).with_filter(LevelFilter::DEBUG))
        .with(analysis_layer.clone())
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(false)
                .with_filter(LevelFilter::INFO),
        );

    (subscriber, provider, analysis_layer)
}

fn simulate_application_work() {
    // Create a root span for the main application work
    let root_span = span!(tracing::Level::INFO, "application_main", version = "1.0.0");
    let _root_guard = root_span.enter();

    info!("Starting application");

    // Simulate some business logic with nested spans
    {
        let auth_span = span!(tracing::Level::DEBUG, "authenticate_user", user_id = 12345);
        let _auth_guard = auth_span.enter();

        debug!("Validating user credentials");

        // Simulate authentication work
        std::thread::sleep(std::time::Duration::from_millis(10));

        info!("User authenticated successfully");
    }

    // Simulate database operations
    {
        let db_span = span!(
            tracing::Level::DEBUG,
            "database_query",
            query = "SELECT * FROM users",
            table = "users"
        );
        let _db_guard = db_span.enter();

        debug!("Executing database query");

        // Nested span for connection management
        {
            let conn_span = span!(tracing::Level::DEBUG, "acquire_connection", pool_size = 10);
            let _conn_guard = conn_span.enter();

            debug!("Acquiring database connection from pool");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        std::thread::sleep(std::time::Duration::from_millis(20));
        info!("Database query completed");
    }

    // Simulate some processing work
    {
        let process_span = span!(
            tracing::Level::DEBUG,
            "process_data",
            records_count = 150,
            batch_size = 50
        );
        let _process_guard = process_span.enter();

        debug!("Processing user data");

        for batch in 1..=3 {
            let batch_span = span!(
                tracing::Level::DEBUG,
                "process_batch",
                batch_number = batch,
                batch_size = 50
            );
            let _batch_guard = batch_span.enter();

            debug!("Processing batch {}", batch);
            std::thread::sleep(std::time::Duration::from_millis(8));
        }

        info!("Data processing completed");
    }

    warn!("Application work completed");
}

fn main() {
    println!(
        "🚀 OpenTelemetryContext Example: Extracting OpenTelemetry Contexts from Separate Layer"
    );
    println!("{}", "=".repeat(80));

    // Setup tracing with our custom layer
    let (subscriber, provider, analysis_layer) = setup_tracing();

    tracing::subscriber::with_default(subscriber, || {
        // Simulate application work that generates spans
        simulate_application_work();
    });

    // Ensure all spans are flushed
    drop(provider);

    // Display the analysis results
    println!("\n📊 Span Analysis Results:");
    println!("{}", "-".repeat(80));

    let results = analysis_layer.get_analysis_results();
    for (i, analysis) in results.iter().enumerate() {
        println!(
            "{}. Span: '{}'\n   Trace ID: {}\n   Span ID: {}\n   Sampled: {}\n",
            i + 1,
            analysis.span_name,
            analysis.trace_id,
            analysis.span_id,
            analysis.is_sampled
        );
    }

    println!(
        "✅ Example completed! Total spans analyzed: {}",
        results.len()
    );
}
