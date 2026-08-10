use std::sync::Arc;

use rookery_discovery::Discovery;
use rookery_fleet::Fleet;

#[derive(Clone)]
pub struct AppState {
    pub fleet: Arc<Fleet>,
    pub discovery: Arc<Discovery>,
    /// The northbound OSC listen address, or `None` when it is switched off.
    /// Shown in the UI so an operator can see at a glance whether a desk can
    /// reach this rookery — and, when it is on, that anything which can reach
    /// the port has full control.
    pub northbound: Option<String>,
    pub northbound_prefix: String,
}
