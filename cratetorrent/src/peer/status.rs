use std::time::Instant;

use crate::{
    avg::SlidingDurationAvg, counter::Counter, Bitfield, PeerId, BLOCK_LEN,
};

/// The status of a peer session, which holds information necessary to modify
/// the state of the session.
pub(super) struct Status {
    /// Information about a peer that is set after a successful handshake.
    pub peer: Option<PeerInfo>,

    /// The current state of the session.
    pub state: State,

    /// Whether the session is in slow start.
    ///
    /// To keep up with the transport layer's slow start algorithm (which unlike
    /// its name, exponentially increases window size), a `peer_session` starts
    /// out in slow start as well, wherein the target request queue size is
    /// increased by one every time one of our requests got served, doubling the
    /// queue size with each round trip.
    pub in_slow_start: bool,

    /// If we're cohked, peer doesn't allow us to download pieces from them.
    pub is_choked: bool,
    /// If we're interested, peer has pieces that we don't have.
    pub is_interested: bool,
    /// If peer is choked, we don't allow them to download pieces from us.
    pub is_peer_choked: bool,
    /// If peer is interested in us, they mean to download pieces that we have.
    pub is_peer_interested: bool,

    /// Counts the bytes sent during protocol chatter.
    pub downloaded_protocol_counter: Counter,
    /// Counts the bytes received during protocol chatter.
    pub uploaded_protocol_counter: Counter,
    // TODO: add counter for uploaded payload bytes once seeding is supported
    /// Counts the downloaded bytes.
    pub downloaded_payload_counter: Counter,

    /// The target request queue size is the number of block requests we keep
    /// outstanding to fully saturate the link.
    ///
    /// Each peer session needs to maintain an "optimal request queue size"
    /// value (approximately the bandwidth-delay product), which is the number
    /// of block requests it keeps outstanding to fully saturate the link.
    ///
    /// This value is derived by collecting a running average of the downloaded
    /// bytes per second, as well as the average request latency, to arrive at
    /// the bandwidth-delay product B x D. This value is recalculated every time
    /// we receive a block, in order to always keep the link fully saturated.
    ///
    /// ```text
    /// queue = download_rate * link_latency / 16 KiB
    /// ```
    ///
    /// Only set once we start downloading.
    // TODO: consider changing this to just usize starting at 0 and reset to
    // 0 once download finishes so that it's easier to deal with it (not having
    // to match on it all the time)
    pub target_request_queue_len: Option<usize>,

    /// The last time some requests were sent to the peer.
    pub last_outgoing_request_time: Option<Instant>,
    /// Updated with the time of receipt of the most recently received requested
    /// block.
    pub last_incoming_block_time: Option<Instant>,
    /// This is the average network round-trip-time between the last issued
    /// a request and receiving the next block.
    ///
    /// Note that it doesn't have to be the same block since peers are not
    /// required to serve our requests in order, so this is more of a general
    /// approximation.
    pub avg_request_rtt: SlidingDurationAvg,
}

impl Status {
    /// When we check whether to exist slow start mode we want to allow for some
    /// error margin. This is because there may be "micro-fluctuations" in the
    /// download rate but per second but over a longer time the download rate
    /// may still be increasing significantly.
    const SLOW_START_ERROR_MARGIN: u64 = 10000;

    /// The target request queue size is set to this value once we are able to start
    /// downloading.
    const START_REQUEST_QUEUE_LEN: usize = 4;

    /// Prepares the for requesting downloads.
    pub fn prepare_for_download(&mut self) {
        debug_assert!(self.is_interested);
        debug_assert!(!self.is_choked);

        self.in_slow_start = true;
        // reset the target request queue size, which will be adjusted as the
        // download progresses
        self.target_request_queue_len = Some(Self::START_REQUEST_QUEUE_LEN);
    }

    /// Updates various statistics around a block download.
    ///
    /// This should be called every time a block is received.
    pub fn update_download_stats(&mut self, block_len: u32) {
        let now = Instant::now();

        // update request time
        if let Some(last_outgoing_request_time) =
            &mut self.last_outgoing_request_time
        {
            let request_rtt = now.duration_since(*last_outgoing_request_time);
            self.avg_request_rtt.update(request_rtt);
        }

        self.downloaded_payload_counter += block_len as u64;
        self.last_incoming_block_time = Some(now);

        // if we're in slow-start mode, we need to increase the target queue
        // size every time a block is received
        if self.in_slow_start {
            if let Some(target_request_queue_len) =
                &mut self.target_request_queue_len
            {
                *target_request_queue_len += 1;
                log::info!(
                    "Request queue incremented in slow-start to {}",
                    *target_request_queue_len
                );
            }
        }
    }

    /// Updates various statistics and session state.
    ///
    /// This should be called every second.
    pub fn tick(&mut self) {
        self.maybe_exit_slow_start();

        self.update_target_request_queue_len();

        self.reset_counters();
    }

    /// Check if we need to exit slow start.
    ///
    /// We leave slow start if the download rate has not increased
    /// significantly since the last round.
    fn maybe_exit_slow_start(&mut self) {
        // this only makes sense if we're not choked
        if !self.is_choked
            && self.in_slow_start
            && self.target_request_queue_len.is_some()
            && self.downloaded_payload_counter.round() > 0
            && self.downloaded_payload_counter.round()
                + Self::SLOW_START_ERROR_MARGIN
                < self.downloaded_payload_counter.avg()
        {
            self.in_slow_start = false;
        }
    }

    /// Adjusts the target request queue size based on the current download
    /// statistics.
    ///
    /// # Important
    ///
    /// This must not be called when the peer is in slow start mode, as in that
    /// case the request queue size is increased by one every time a block is
    /// received. This function is for after the slow start phase.
    fn update_target_request_queue_len(&mut self) {
        if let Some(target_request_queue_len) =
            &mut self.target_request_queue_len
        {
            let prev_queue_len = *target_request_queue_len;

            // this is only applicable if we're not in slow start, as in slow
            // start mode the request queue is increased with each incoming
            // block
            if !self.in_slow_start {
                let download_rate = self.downloaded_payload_counter.avg();
                // guard against integer truncation and round up as
                // overestimating the link capacity is cheaper than
                // underestimating it
                *target_request_queue_len =
                    ((download_rate + (BLOCK_LEN - 1) as u64)
                        / BLOCK_LEN as u64) as usize;
            }

            // make sure the target doesn't go below 1
            // TODO: make this configurable and also enforce an upper bound
            if *target_request_queue_len < 1 {
                *target_request_queue_len = 1;
            }

            if prev_queue_len != *target_request_queue_len {
                log::info!(
                    "Request queue changed from {} to {}",
                    prev_queue_len,
                    *target_request_queue_len
                );
            }
        }
    }

    /// Marks the end of the round for the various throughput rate counters.
    fn reset_counters(&mut self) {
        for counter in [
            &mut self.downloaded_payload_counter,
            &mut self.uploaded_protocol_counter,
            &mut self.downloaded_protocol_counter,
        ]
        .iter_mut()
        {
            counter.reset();
        }
    }
}

impl Default for Status {
    /// By default, both sides of the connection start off as choked and not
    /// interested in the other.
    fn default() -> Self {
        Self {
            state: State::default(),
            in_slow_start: false,
            is_choked: true,
            is_interested: false,
            is_peer_choked: true,
            is_peer_interested: false,
            downloaded_protocol_counter: Counter::default(),
            uploaded_protocol_counter: Counter::default(),
            downloaded_payload_counter: Counter::default(),
            target_request_queue_len: None,
            last_outgoing_request_time: None,
            last_incoming_block_time: None,
            avg_request_rtt: SlidingDurationAvg::default(),
            peer: None,
        }
    }
}

/// Information about the peer we're connected to.
#[derive(Debug)]
pub(super) struct PeerInfo {
    /// Peer's 20 byte BitTorrent id.
    pub id: PeerId,
    /// All pieces peer has, updated when it announces to us a new piece.
    pub pieces: Option<Bitfield>,
}

/// At any given time, a connection with a peer is in one of the below states.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum State {
    /// The peer connection has not yet been connected or it had been connected
    /// before but has been stopped.
    Disconnected,
    /// The state during which the TCP connection is established.
    Connecting,
    /// The state after establishing the TCP connection and exchanging the
    /// initial BitTorrent handshake.
    Handshaking,
    /// This state is optional, it is used to verify that the bitfield exchange
    /// occurrs after the handshake and not later. It is set once the handshakes
    /// are exchanged and changed as soon as we receive the bitfield or the the
    /// first message that is not a bitfield. Any subsequent bitfield messages
    /// are rejected and the connection is dropped, as per the standard.
    AvailabilityExchange,
    /// This is the normal state of a peer session, in which any messages, apart
    /// from the 'handshake' and 'bitfield', may be exchanged.
    Connected,
}

/// The default (and initial) state of a peer session is `Disconnected`.
impl Default for State {
    fn default() -> Self {
        Self::Disconnected
    }
}
