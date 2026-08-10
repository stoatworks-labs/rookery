//! The northbound OSC listener: one cue in, N datagrams out.
//!
//! Everything a desk sends is parsed against the registry *as it is at that
//! moment* and then fanned out. A parse failure is logged with the address
//! and the reason and nothing is sent — a cue that half-applies across a
//! fleet is worse than one that visibly does nothing.

use std::sync::Arc;

use rookery_fleet::{parse_northbound, Fleet};
use rookery_osc::OscReceiver;

pub async fn spawn(
    bind: &str,
    prefix: String,
    fleet: Arc<Fleet>,
) -> anyhow::Result<std::net::SocketAddr> {
    let mut receiver = OscReceiver::bind(bind).await?;
    let local = receiver.local_addr;

    tokio::spawn(async move {
        while let Some(received) = receiver.messages.recv().await {
            let address = received.message.address.clone();
            let cue =
                match parse_northbound(&prefix, &address, &received.message.args, fleet.registry())
                {
                    Ok(cue) => cue,
                    Err(e) => {
                        tracing::warn!(from = %received.from, "northbound: {e:#}");
                        continue;
                    }
                };

            tracing::info!(
                from = %received.from,
                %address,
                "northbound cue: {} -> {}",
                cue.command.summary(),
                cue.target.describe()
            );
            let fanout = fleet.send(&cue.target, &cue.command).await;
            if !fanout.fully_sent() {
                // Loud, because the desk will never hear about it: OSC has no
                // reply path, so this log line and the UI are the only places
                // a failed cue can surface at all.
                tracing::warn!(
                    %address,
                    "northbound cue did not fully send: {}",
                    fanout.summary()
                );
            }
        }
        tracing::error!("northbound OSC receiver stopped");
    });

    Ok(local)
}
