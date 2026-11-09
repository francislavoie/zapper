//! What an HDHomeRun account is: where the tuner is, and optionally which
//! XMLTV guide to use instead of SiliconDust's.

use std::fmt;

#[derive(Clone, Default, PartialEq, Eq)]
pub struct HdhrAccount {
    /// The tuner's address as typed: `192.168.1.50`, `hdhomerun.local`,
    /// `http://10.0.0.7:80`. Read by [`super::device::base_url`].
    pub device: String,
    /// XMLTV locations to use for the guide, in place of SiliconDust's. Empty
    /// means SiliconDust's free guide for the tuner's own lineup, which is
    /// what an owner gets from the HDHomeRun app; a list is for somebody who
    /// already runs a grabber and prefers its listings.
    pub epg_urls: Vec<String>,
}

impl fmt::Debug for HdhrAccount {
    /// The device address is a LAN address and fine to log. A guide URL may
    /// carry a key (Schedules Direct exports, paid grabbers), so it goes
    /// through the same redaction as a playlist's.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HdhrAccount")
            .field("device", &self.device)
            .field(
                "epg_urls",
                &self
                    .epg_urls
                    .iter()
                    .map(|u| crate::m3u::redact::redact_url(u).0)
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_output_never_carries_a_guide_key() {
        let a = HdhrAccount {
            device: "192.168.1.50".into(),
            epg_urls: vec!["http://grabber.example/guide.xml?key=hunter2".into()],
        };
        let s = format!("{a:?}");
        assert!(!s.contains("hunter2"), "{s}");
        assert!(s.contains("192.168.1.50"), "{s}");
    }
}
