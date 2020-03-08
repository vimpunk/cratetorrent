use crate::Bitfield;

// Metadata about a piece relevant for the piece picker.
#[derive(Clone, Copy, Default)]
struct Piece {
    // The frequency of this piece in the torrent swarm.
    frequency: usize,
    // Whether we have already picked this piece and are currently downloading
    // it. This flag is set to true when the piece is picked.
    //
    // This is to prevent picking the same piece we are already downloading in
    // the scenario in which we want to pick a new piece before the already
    // downloadng piece finishes. Not having this check would lead us to always
    // pick this piece until we tell the piece picker that we have it and thus
    // wouldn't be able to download multiple pieces simultaneously (an important
    // optimizaiton step).
    is_pending: bool,
}

pub struct PiecePicker {
    // Represents the pieces that we have downloaded.
    //
    // The bitfield is pre-allocated to the number of pieces in the torrent and
    // each field that we have is set to true.
    own_pieces: Bitfield,
    // We collect metadata about pieces in the torrent swarm in this vector.
    //
    // The vector is pre-allocated to the number of pieces in the torrent.
    pieces: Vec<Piece>,
}

impl PiecePicker {
    pub fn new(piece_count: usize) -> Self {
        let mut pieces = Vec::new();
        pieces.resize_with(piece_count, Piece::default);
        Self {
            own_pieces: Bitfield::repeat(false, piece_count),
            pieces,
        }
    }

    /// Returns the first piece that we don't yet have and isn't already being
    /// downloaded, or None, if no piece can be picked at this time.
    pub fn pick_piece(&mut self) -> Option<u32> {
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
                return Some(index as u32);
            }
        }
        // no piece could be picked
        None
    }

    /// Registers the avilability of a peer's pieces.
    pub fn register_availability(&mut self, pieces: &Bitfield) {
        log::trace!("Registering piece availability: {}", pieces);
        // TODO: this should possibly not be a debug assert and we should return
        // an error
        debug_assert!(pieces.len() == self.own_pieces.len());
        for (index, has_piece) in pieces.iter().enumerate() {
            // increase frequency count for this piece if peer has it
            if *has_piece {
                self.pieces[index].frequency += 1;
            }
        }
    }

    /// Registers the avilability of a single new piece of a peer.
    pub fn register_piece_availability(&mut self, index: usize) {
        log::trace!("Registering piece {} availability", index);
        // TODO: this should possibly not be a debug assert and we should return
        // an error
        debug_assert!(index < self.own_pieces.len());
        // increase frequency count for this piece
        self.pieces[index].frequency += 1;
    }

    /// Tells the piece picker that we have downloaded the piece at the given
    /// index.
    pub fn received_piece(&mut self, index: usize) {
        log::trace!("Registering received piece {}", index);
        // TODO: this should possibly not be a debug assert and we should return
        // an error
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    // Tests that repeatedly requesting as many pieces as are in the piece
    // picker returns all pieces, none of them previously picked.
    #[test]
    fn test_pick_all_pieces() {
        let piece_count = 15;
        let mut piece_picker = PiecePicker::new(piece_count);
        let available_pieces = Bitfield::repeat(true, piece_count);
        piece_picker.register_availability(&available_pieces);

        let mut picked = HashSet::with_capacity(piece_count);
        for index in 0..piece_count {
            let pick = piece_picker.pick_piece();
            // for now we assert that we pick pieces in sequential order, but
            // later, when we add different algorithms, this line has to change
            assert_eq!(pick, Some(index as u32));
            let pick = pick.unwrap();
            // assert that this piece hasn't been picked before
            assert!(!picked.contains(&pick));
            // mark piece as picked
            picked.insert(pick);
        }

        // assert that we picked all pieces
        assert_eq!(picked.len(), piece_count);
    }
}
