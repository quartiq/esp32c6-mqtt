use embassy_futures::select::{Either, select};
use miniconf_mqtt::{Error, Miniconf, Service, ServiceEvent, Startup};
use minimq::{Connection, Io, PubError, Publication, ResourceError};

use crate::{Settings, Shared};

/// Serve configuration and telemetry without cancelling protocol follow-ups.
pub async fn serve<IO: Io>(
    connection: &mut Connection<'_, '_, IO>,
    miniconf: &mut Miniconf<Settings>,
    settings: &mut Settings,
    shared: &Shared,
    topic: &str,
) -> Result<(), Error<IO::Error>> {
    // This connection owns a new Service; repair mirrors of any abandoned SETs.
    let mut startup = Startup::connected(miniconf);
    let mut service = Service::<4>::new();
    loop {
        let ready = startup.step(miniconf, connection, settings).await?;
        if ready {
            service.step(miniconf, connection, settings).await?;
        }
        if ready
            && connection.session().is_publish_quiescent()
            && let Some(telemetry) = shared.telemetry.try_take()
        {
            connection
                .publish(Publication::new(topic, |buffer: &mut [u8]| {
                    serde_json_core::to_slice(&telemetry, buffer)
                }))
                .await
                .map_err(|e| match e {
                    PubError::Session(e) => Error::Mqtt(e),
                    PubError::Payload(_) => {
                        Error::Mqtt(minimq::Error::Resource(ResourceError::BufferTooSmall))
                    }
                })?;
        }
        // Only poll is cancelled. Service/startup/publish complete their local step.
        // When full, wait for protocol progress instead of spinning on a pending sample.
        let inbound = if ready && service.is_empty() && connection.session().is_publish_quiescent()
        {
            match select(connection.poll(), shared.telemetry.wait()).await {
                Either::First(result) => result?,
                Either::Second(telemetry) => {
                    shared.telemetry.signal(telemetry);
                    continue;
                }
            }
        } else {
            connection.poll().await?
        };
        if let Some(inbound) = inbound
            && matches!(
                service.handle(miniconf, settings, &inbound),
                ServiceEvent::Changed(_)
            )
        {
            shared.interval.signal(settings.interval);
        }
    }
}
