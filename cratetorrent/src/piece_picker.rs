use crate::{error::*, Bitfield};

pub(crate) struct PiecePicker {
    /// Represents the pieces that we have downloaded.
    ///
    /// The bitfield is pre-allocated to the number of pieces in the torrent and
    /// each field that we have is set to true.
    own_pieces: Bitfield,
    /// We collect metadata about pieces in the torrent swarm in this vector.
    ///
    /// The vector is pre-allocated to the number of pieces in the torrent.
    pieces: Vec<Piece>,
}

impl PiecePicker {
    /// Creates a new empty piece picker for the given number of pieces.
    ///
    /// Empty means that we don't have any of the pieces yet, so piece picker
    /// will pick all of them, if available from our peers.
    pub fn new(piece_count: usize) -> Self {
        let mut pieces = Vec::new();
        pieces.resize_with(piece_count, Piece::default);
        Self {
            own_pieces: Bitfield::repeat(false, piece_count),
            pieces,
        }
    }

    /// Returns the number of missing pieces that are needed to complete the
    /// download.
    pub fn count_missing_pieces(&self) -> usize {
        self.own_pieces.count_zeros()
    }

    /// Returns the first piece that we don't yet have and isn't already being
    /// downloaded, or None, if no piece can be picked at this time.
    pub fn pick_piece(&mut self) -> Option<usize> {
        log::trace!("Picking next piece");

        for (index, have_piece) in self.own_pieces.iter().enumerate() {
            // only consider this piece if we don't have it and if we are not
            // already downloading it (whether it's not pending)
            debug_assert!(index < self.pieces.len());
            let piece = &mut self.pieces[index];
            if !have_piece && piece.frequency > 0 && !piece.is_pending {
                // set pending flag on piece so that this piece is not picked
                // again (see note on field)
                piece.is_pending = true;
                log::trace!("Picked piece {}", index);
                return Some(index);
            }
        }

        // no piece could be picked
        log::trace!("Could not pick piece");
        None
    }

    /// Registers the avilability of a peer's pieces and returns whether we're
    /// interested in peer's pieces.
    pub fn register_availability(&mut self, pieces: &Bitfield) -> Result<bool> {
        log::trace!("Registering piece availability: {}", pieces);

        if pieces.len() != self.own_pieces.len() {
            log::warn!("Peer sent us bitfield with disparate number of pieces");
            return Err(Error::InvalidBitfield);
        }

        let mut interested = false;
        for (index, (have_piece, peer_has_piece)) in
            self.own_pieces.iter().zip(pieces.iter()).enumerate()
        {
            // increase frequency count for this piece if peer has it
            if *peer_has_piece {
                self.pieces[index].frequency += 1;
                // if we don't have at least one piece peer has, we're
                // interested
                if !have_piece {
                    interested = true;
                }
            }
        }

        Ok(interested)
    }

    /// Tells the piece picker that we have downloaded the piece at the given
    /// index.
    pub fn received_piece(&mut self, index: usize) {
        log::trace!("Registering received piece {}", index);

        // we assert here as this method is only called by internal methods on
        // piece completion, meaning the piece must exist (we can't download an
        // invalid piece)
        debug_assert!(index < self.own_pieces.len());

        // register owned piece
        self.own_pieces.set(index, true);
        // also set that this piece is no longer pending (even though we won't
        // be downloading it anymore, later we may re-download a piece in which
        // case not resetting the flag would cause us to never pick that piece
        // again)
        self.pieces[index].is_pending = false;
    }
}

/// Metadata about a piece relevant for the piece picker.
#[derive(Clone, Copy, Default)]
struct Piece {
    /// The frequency of this piece in the torrent swarm.
    frequency: usize,
    /// Whether we have already picked this piece and are currently downloading
    /// it. This flag is set to true when the piece is picked.
    ///
    /// This is to prevent picking the same piece we are already downloading in
    /// the scenario in which we want to pick a new piece before the already
    /// downloadng piece finishes. Not having this check would lead us to always
    /// pick this piece until we tell the piece picker that we have it and thus
    /// wouldn't be able to download multiple pieces simultaneously (an
    /// important optimizaiton step).
    is_pending: bool,
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    // Tests that repeatedly requesting as many pieces as are in the piece
    // picker returns all pieces, none of them previously picked.
    #[test]
    fn test_pick_all_pieces() {
        let piece_count = 15;
        let mut piece_picker = PiecePicker::new(piece_count);
        let available_pieces = Bitfield::repeat(true, piece_count);
        piece_picker.register_availability(&available_pieces);

        // save picked pieces
        let mut picked = HashSet::with_capacity(piece_count);

        // pick all pieces one by one
        for index in 0..piece_count {
            let pick = piece_picker.pick_piece();
            // for now we assert that we pick pieces in sequential order, but
            // later, when we add different algorithms, this line has to change
            assert_eq!(pick, Some(index));
            let pick = pick.unwrap();
            // assert that this piece hasn't been picked before
            assert!(!picked.contains(&pick));
            // mark piece as picked
            picked.insert(pick);
        }

        // assert that we picked all pieces
        assert_eq!(picked.len(), piece_count);
    }

    // Tests registering a received piece causes the piece picker to not pick
    // that piece again.
    #[test]
    fn test_received_piece() {
        let piece_count = 15;
        let mut piece_picker = PiecePicker::new(piece_count);
        let available_pieces = Bitfield::repeat(true, piece_count);
        piece_picker.register_availability(&available_pieces);
        assert!(piece_picker.own_pieces.not_any());

        // mark pieces as received
        let owned_pieces = [3, 10, 5];
        for index in owned_pieces.iter() {
            piece_picker.received_piece(*index);
            assert!(piece_picker.own_pieces[*index]);
        }
        assert!(!piece_picker.own_pieces.is_empty());

        // request pieces to pick next and make sure the ones we already have
        // are not picked
        for _ in 0..piece_count - owned_pieces.len() {
            let pick = piece_picker.pick_piece().unwrap();
            // assert that it's not a piece we already have
            assert!(owned_pieces.iter().all(|owned| *owned != pick));
        }
    }

    // Tests that the piece picker correctly determines whether we are
    // interested in a variety of piece sets.
    #[test]
    fn test_is_interested() {
        // empty piece picker
        let piece_count = 15;
        let mut piece_picker = PiecePicker::new(piece_count);

        // we are interested if peer has all pieces
        let available_pieces = Bitfield::repeat(true, piece_count);
        assert!(piece_picker
            .register_availability(&available_pieces)
            .unwrap());

        // we are also interested if peer has at least a single piece
        let mut available_pieces = Bitfield::repeat(false, piece_count);
        available_pieces.set(0, true);
        assert!(piece_picker
            .register_availability(&available_pieces)
            .unwrap());

        // half full piece picker
        let piece_count = 15;
        let mut piece_picker = PiecePicker::new(piece_count);
        for index in 0..8 {
            piece_picker.received_piece(index);
        }

        // we are not interested in peer that has the same pieces we do
        let mut available_pieces = Bitfield::repeat(false, piece_count);
        for index in 0..8 {
            available_pieces.set(index, true);
        }
        assert!(!piece_picker
            .register_availability(&available_pieces)
            .unwrap());

        // we are interested in peer that has at least a single piece we don't
        let mut available_pieces = Bitfield::repeat(false, piece_count);
        for index in 0..9 {
            available_pieces.set(index, true);
        }
        assert!(piece_picker
            .register_availability(&available_pieces)
            .unwrap());

        // full piece picker
        let piece_count = 15;
        let mut piece_picker = PiecePicker::new(piece_count);
        for index in 0..piece_count {
            piece_picker.received_piece(index);
        }

        // we are not interested in any pieces since we own all of them
        assert!(!piece_picker
            .register_availability(&available_pieces)
            .unwrap());
    }
}
