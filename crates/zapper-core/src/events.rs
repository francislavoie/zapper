//! One bus for what the core tells whoever is listening: the recording
//! service's browsers and any front end. A broadcast channel,
//! so a producer never waits for a slow listener and never fails for lack of one.

use std::sync::OnceLock;

use tokio::sync::broadcast::{self, error::RecvError};

use crate::api::types::{
    events, ChannelQualityDto, PlayerOptionsDto, PlayerStateDto, SyncStatusDto, UpdateStatusDto,
};

/// What goes out on the bus. Host events (media keys, the menu) are the
/// window's own and stay out.
#[derive(Debug, Clone)]
pub enum Event {
    SyncStatus(SyncStatusDto),
    PlayerState(PlayerStateDto),
    PlayerOptions(PlayerOptionsDto),
    ChannelQuality(ChannelQualityDto),
    Update(UpdateStatusDto),
}

impl Event {
    /// The name the webview listens under.
    pub fn name(&self) -> &'static str {
        match self {
            Event::SyncStatus(_) => events::SYNC_STATUS,
            Event::PlayerState(_) => events::PLAYER_STATE,
            Event::PlayerOptions(_) => events::PLAYER_OPTIONS,
            Event::ChannelQuality(_) => events::CHANNEL_QUALITY,
            Event::Update(_) => events::UPDATE,
        }
    }
}

pub struct EventBus {
    tx: broadcast::Sender<Event>,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        Self {
            tx: broadcast::channel(capacity).0,
        }
    }

    /// Sends to every current subscriber; nobody listening is fine.
    pub fn emit(&self, e: Event) {
        let _ = self.tx.send(e);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }
}

/// The process-wide bus.
pub fn bus() -> &'static EventBus {
    static BUS: OnceLock<EventBus> = OnceLock::new();
    BUS.get_or_init(|| EventBus::new(256))
}

/// The next player state, or on a lagged receiver the newest one still
/// held: a listener that fell behind wants where the player is now, not
/// every step it missed. Every other kind of event is skipped, so a lag
/// whose last event is, say, a channel's quality still ends on the state
/// before it (a state is sent only when it changes, so missing the last one
/// would leave the listener on a stale one). Only for consumers that want
/// the player's state alone. `None` once the bus is gone.
pub async fn recv_latest_player_state(
    rx: &mut broadcast::Receiver<Event>,
) -> Option<PlayerStateDto> {
    loop {
        match rx.recv().await {
            Ok(Event::PlayerState(s)) => return Some(s),
            Ok(_) => continue,
            Err(RecvError::Closed) => return None,
            Err(RecvError::Lagged(_)) => {
                let mut latest = None;
                loop {
                    match rx.try_recv() {
                        Ok(Event::PlayerState(s)) => latest = Some(s),
                        Ok(_) => {}
                        Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                        Err(_) => break,
                    }
                }
                if latest.is_some() {
                    return latest;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::{SyncPhase, UpdateStateDto};

    fn state(pos: f64) -> Event {
        Event::PlayerState(PlayerStateDto {
            buffered_back: pos,
            ..Default::default()
        })
    }

    fn pos(e: &Event) -> f64 {
        match e {
            Event::PlayerState(s) => s.buffered_back,
            _ => panic!("not a player state"),
        }
    }

    #[tokio::test]
    async fn emit_reaches_every_subscriber() {
        let bus = EventBus::new(8);
        let (mut a, mut b) = (bus.subscribe(), bus.subscribe());
        bus.emit(state(3.0));
        assert_eq!(pos(&a.recv().await.unwrap()), 3.0);
        assert_eq!(pos(&b.recv().await.unwrap()), 3.0);
    }

    #[test]
    fn emit_without_subscribers_is_fine() {
        EventBus::new(8).emit(state(1.0));
    }

    #[tokio::test]
    async fn a_lagging_subscriber_gets_the_latest_state() {
        let bus = EventBus::new(4);
        let mut rx = bus.subscribe();
        for i in 0..10 {
            bus.emit(state(i as f64));
        }
        // Another kind of event after the last state: ignored, not taken
        // for the newest.
        bus.emit(quality());
        let s = recv_latest_player_state(&mut rx).await.unwrap();
        assert_eq!(s.buffered_back, 9.0);
        // Without a lag, other kinds are skipped on the way to the next state.
        bus.emit(quality());
        bus.emit(state(10.0));
        let s = recv_latest_player_state(&mut rx).await.unwrap();
        assert_eq!(s.buffered_back, 10.0);
    }

    fn quality() -> Event {
        Event::ChannelQuality(ChannelQualityDto {
            stream_id: 1,
            quality: None,
        })
    }

    #[test]
    fn event_names_match_today() {
        let cases = [
            (
                Event::SyncStatus(SyncStatusDto {
                    phase: SyncPhase::Idle,
                    message: String::new(),
                    channels_total: 0,
                    last_synced: None,
                }),
                events::SYNC_STATUS,
            ),
            (state(0.0), events::PLAYER_STATE),
            (
                Event::PlayerOptions(PlayerOptionsDto::default()),
                events::PLAYER_OPTIONS,
            ),
            (
                Event::ChannelQuality(ChannelQualityDto {
                    stream_id: 1,
                    quality: None,
                }),
                events::CHANNEL_QUALITY,
            ),
            (
                Event::Update(UpdateStatusDto {
                    current: String::new(),
                    kind: String::new(),
                    last_checked: None,
                    state: UpdateStateDto::Idle,
                }),
                events::UPDATE,
            ),
        ];
        for (e, name) in cases {
            assert_eq!(e.name(), name);
        }
    }
}
