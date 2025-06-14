use std::{
    error::Error as StdError,
    fmt::{Debug, Display},
    io::Write,
    thread,
    time::{Duration, SystemTime},
};

use opentelemetry::global;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::{self as sdk, error::OTelSdkResult, trace::SpanExporter};
use tracing::{error, instrument, span, trace, warn};
use tracing_subscriber::prelude::*;

#[derive(Debug)]
enum Error {
    ErrorQueryPassed,
}

impl StdError for Error {}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::ErrorQueryPassed => write!(f, "Encountered the error flag in the query"),
        }
    }
}

#[instrument(err)]
fn failable_work(fail: bool) -> Result<&'static str, Error> {
    span!(tracing::Level::INFO, "expensive_step_1")
        .in_scope(|| thread::sleep(Duration::from_millis(25)));
    span!(tracing::Level::INFO, "expensive_step_2")
        .in_scope(|| thread::sleep(Duration::from_millis(25)));

    if fail {
        return Err(Error::ErrorQueryPassed);
    }
    Ok("success")
}

#[instrument(err)]
fn double_failable_work(fail: bool) -> Result<&'static str, Error> {
    span!(tracing::Level::INFO, "expensive_step_1")
        .in_scope(|| thread::sleep(Duration::from_millis(25)));
    span!(tracing::Level::INFO, "expensive_step_2")
        .in_scope(|| thread::sleep(Duration::from_millis(25)));
    error!(error = "test", "hello");
    if fail {
        return Err(Error::ErrorQueryPassed);
    }
    Ok("success")
}

fn main() -> Result<(), Box<dyn StdError + Send + Sync + 'static>> {
    let builder = sdk::trace::SdkTracerProvider::builder().with_simple_exporter(WriterExporter);
    let provider = builder.build();
    let tracer = provider.tracer("opentelemetry-write-exporter");

    global::set_tracer_provider(provider.clone());

    let opentelemetry = tracing_opentelemetry::layer().with_tracer(tracer);
    tracing_subscriber::registry()
        .with(opentelemetry)
        .try_init()?;

    {
        let root = span!(tracing::Level::INFO, "app_start", work_units = 2);
        let _enter = root.enter();

        let work_result = failable_work(false);

        trace!("status: {}", work_result.unwrap());
        let work_result = failable_work(true);

        trace!("status: {}", work_result.err().unwrap());
        warn!("About to exit!");

        let _ = double_failable_work(true);
    } // Once this scope is closed, all spans inside are closed as well

    // Shut down the current tracer provider. This will invoke the shutdown
    // method on all span processors. span processors should export remaining
    // spans before return.
    provider.shutdown().unwrap();

    Ok(())
}

#[derive(Debug)]
struct WriterExporter;

impl SpanExporter for WriterExporter {
    async fn export(&self, batch: Vec<sdk::trace::SpanData>) -> OTelSdkResult {
        let mut writer = std::io::stdout();
        for span in batch {
            writeln!(writer, "{}", SpanData(span)).unwrap();
        }
        writeln!(writer).unwrap();

        OTelSdkResult::Ok(())
    }
}

struct SpanData(sdk::trace::SpanData);
impl Display for SpanData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Span: \"{}\"", self.0.name)?;
        match &self.0.status {
            opentelemetry::trace::Status::Unset => {}
            opentelemetry::trace::Status::Error { description } => {
                writeln!(f, "- Status: Error")?;
                writeln!(f, "- Error: {description}")?
            }
            opentelemetry::trace::Status::Ok => writeln!(f, "- Status: Ok")?,
        }
        writeln!(
            f,
            "- Start: {}",
            self.0
                .start_time
                .duration_since(SystemTime::UNIX_EPOCH)
                .expect("start time is before the unix epoch")
                .as_secs()
        )?;
        writeln!(
            f,
            "- End: {}",
            self.0
                .end_time
                .duration_since(SystemTime::UNIX_EPOCH)
                .expect("end time is before the unix epoch")
                .as_secs()
        )?;
        writeln!(f, "- Resource:")?;
        for kv in self.0.attributes.iter() {
            writeln!(f, "  - {}: {}", kv.key, kv.value)?;
        }
        writeln!(f, "- Attributes:")?;
        for kv in self.0.attributes.iter() {
            writeln!(f, "  - {}: {}", kv.key, kv.value)?;
        }

        writeln!(f, "- Events:")?;
        for event in self.0.events.iter() {
            if let Some(error) =
                event
                    .attributes
                    .iter()
                    .fold(Option::<String>::None, |mut acc, d| {
                        if let Some(mut acc) = acc.take() {
                            use std::fmt::Write;
                            let _ = write!(acc, ", {}={}", d.key, d.value);
                            Some(acc)
                        } else {
                            Some(format!("{} = {}", d.key, d.value))
                        }
                    })
            {
                writeln!(f, "  - \"{}\" {{{error}}}", event.name)?;
            } else {
                writeln!(f, "  - \"{}\"", event.name)?;
            }
        }
        writeln!(f, "- Links:")?;
        for link in self.0.links.iter() {
            writeln!(f, "  - {:?}", link)?;
        }
        Ok(())
    }
}
