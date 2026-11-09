//! zapper-server, the recording service, on zapper-core's catalogue, sources
//! and guide: it records, shares the account's connections between
//! recordings and browsers watching, and serves the browser interface with
//! the API it reads. A headless box needs no libmpv: this crate does not
//! depend on zapper-mpv. docs/architecture.md has the picture,
//! docs/recording.md the service.

pub mod recording;
