use std::fmt;

use bounded_vec_deque::BoundedVecDeque;
use chrono::{DateTime, FixedOffset};
use enclose::enclose;
use warpui::{Entity, ModelContext, SingletonEntity};

/// Maximum number of network log items retained in memory.
const NETWORK_LOGGING_MAX_ITEMS: usize = 50;

/// Upper bound on the channel between HTTP client hooks and the in-memory model.
const NETWORK_LOGGING_MAX_QUEUE_SIZE: usize = 100;

/// In-memory store of recent requests made by local HTTP clients.
pub struct NetworkLogModel {
    items: BoundedVecDeque<NetworkLogItem>,
}

impl Default for NetworkLogModel {
    fn default() -> Self {
        Self {
            items: BoundedVecDeque::new(NETWORK_LOGGING_MAX_ITEMS),
        }
    }
}

impl NetworkLogModel {
    /// Creates the model and installs non-blocking logging hooks on the supplied local clients.
    pub fn new<'a>(
        http_clients: impl IntoIterator<Item = &'a mut http_client::Client>,
        ctx: &mut ModelContext<Self>,
    ) -> Self {
        let (tx, rx) = async_channel::bounded::<NetworkLogItem>(NETWORK_LOGGING_MAX_QUEUE_SIZE);

        ctx.spawn_stream_local(
            rx,
            |model, item, ctx| model.push(item, ctx),
            |_model, _ctx| {},
        );

        for client in http_clients {
            client.set_before_request_fn(Box::new(
                enclose!((tx) move |request, serialized_payload| {
                    if !tx.is_closed()
                        && let Err(error) = tx.try_send(NetworkLogItem::request(
                            request,
                            serialized_payload.clone(),
                            chrono::Local::now().fixed_offset(),
                        )) {
                            log::error!("Error sending request to the network log: {error}");
                        }
                }),
            ));

            client.set_after_response_fn(Box::new(enclose!((tx) move |response| {
                if !tx.is_closed()
                    && let Err(error) = tx.try_send(NetworkLogItem::response(
                        response,
                        chrono::Local::now().fixed_offset(),
                    )) {
                        log::error!("Error sending response to the network log: {error}");
                    }
            })));
        }

        Self::default()
    }

    pub fn push(&mut self, item: NetworkLogItem, ctx: &mut ModelContext<Self>) {
        let _evicted = self.items.push_back(item);
        ctx.notify();
    }

    pub fn snapshot_text(&self) -> String {
        let mut out = String::new();
        for (index, item) in self.items.iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            out.push_str(&item.0);
        }
        out
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.items.len()
    }
}

impl Entity for NetworkLogModel {
    type Event = ();
}

impl SingletonEntity for NetworkLogModel {}

#[derive(Clone, Debug)]
pub struct NetworkLogItem(String);

impl NetworkLogItem {
    pub fn request(
        request: &reqwest::Request,
        serialized_payload: Option<String>,
        timestamp: DateTime<FixedOffset>,
    ) -> Self {
        Self(format!(
            "[{}]: {:?}{}",
            timestamp.format("%Y-%m-%d %H:%M:%S,%3f"),
            request,
            serialized_payload.map_or("".to_owned(), |payload| format!("\nBody {payload}"))
        ))
    }

    pub fn response(response: &reqwest::Response, timestamp: DateTime<FixedOffset>) -> Self {
        Self(format!(
            "[{}]: {:?}",
            timestamp.format("%Y-%m-%d %H:%M:%S,%3f"),
            response
        ))
    }

    #[cfg(test)]
    pub fn from_string(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

impl fmt::Display for NetworkLogItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
#[path = "network_logging_tests.rs"]
mod tests;
