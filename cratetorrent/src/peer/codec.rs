use crate::Bitfield;
use bytes::{Buf, BufMut, BytesMut};
use std::convert::TryFrom;
use std::io;
use tokio_util::codec::{Decoder, Encoder};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Handshake {
    /// The protocol string, which must equal "BitTorrent protocol", as
    /// otherwise the connetion is aborted.
    pub prot: [u8; 19],
    /// A reserved field, currently all zeros. This is where the client's
    /// supported extensions are announced.
    pub reserved: [u8; 8],
    /// The torrent's SHA1 info hash, used to identify the torrent in the
    /// handshake and to verify the peer.
    pub info_hash: [u8; 20],
    /// The arbitrary peer id, usually used to identify the torrent client.
    pub peer_id: [u8; 20],
}

impl Handshake {
    pub fn new(info_hash: [u8; 20], peer_id: [u8; 20]) -> Self {
        let mut prot = [0; 19];
        prot.copy_from_slice(PROTOCOL_STRING.as_bytes());
        Self {
            prot,
            reserved: [0; 8],
            info_hash,
            peer_id,
        }
    }
}

#[repr(u8)]
#[derive(Copy, Clone, Debug)]
pub enum MessageId {
    Choke = 0,
    Unchoke = 1,
    Interested = 2,
    NotInterested = 3,
    Have = 4,
    Bitfield = 5,
    Request = 6,
    Block = 7,
    Cancel = 8,
}

impl TryFrom<u8> for MessageId {
    type Error = io::Error;
    fn try_from(k: u8) -> Result<Self, Self::Error> {
        use MessageId::*;
        match k {
            k if k == Choke as u8 => Ok(Choke),
            k if k == Unchoke as u8 => Ok(Unchoke),
            k if k == Interested as u8 => Ok(Interested),
            k if k == NotInterested as u8 => Ok(NotInterested),
            k if k == Have as u8 => Ok(Have),
            k if k == Bitfield as u8 => Ok(Bitfield),
            k if k == Request as u8 => Ok(Request),
            k if k == Block as u8 => Ok(Block),
            k if k == Cancel as u8 => Ok(Cancel),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Unknown message id",
            )),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    KeepAlive,
    Bitfield(Bitfield),
    Choke,
    Unchoke,
    Interested,
    NotInterested,
    Have {
        piece_index: u32,
    },
    Request {
        piece_index: u32,
        offset: u32,
        length: u32,
    },
    Block {
        piece_index: u32,
        offset: u32,
        block: Vec<u8>,
    },
    Cancel {
        piece_index: u32,
        offset: u32,
        length: u32,
    },
}

pub(crate) const PROTOCOL_STRING: &str = "BitTorrent protocol";

pub struct HandshakeCodec;

impl Encoder for HandshakeCodec {
    type Item = Handshake;
    type Error = io::Error;

    fn encode(
        &mut self,
        handshake: Handshake,
        buf: &mut BytesMut,
    ) -> io::Result<()> {
        let Handshake {
            prot,
            reserved,
            info_hash,
            peer_id,
        } = handshake;

        // protocol length prefix
        debug_assert_eq!(prot.len(), 19);
        buf.put_u8(prot.len() as u8);
        // we should only be sending the bittorrent protocol string
        debug_assert_eq!(prot, PROTOCOL_STRING.as_bytes());
        // payload
        buf.extend_from_slice(&prot);
        buf.extend_from_slice(&reserved);
        buf.extend_from_slice(&info_hash);
        buf.extend_from_slice(&peer_id);

        Ok(())
    }
}

impl Decoder for HandshakeCodec {
    type Item = Handshake;
    type Error = io::Error;

    fn decode(&mut self, buf: &mut BytesMut) -> io::Result<Option<Handshake>> {
        if buf.is_empty() {
            return Ok(None);
        }

        let prot_len = buf.get_u8() as usize;
        if prot_len != PROTOCOL_STRING.as_bytes().len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Handshake must have the string \"BitTorrent protocol\"",
            ));
        }

        // check that we got the full payload in the buffer
        let payload_len = prot_len + 8 + 20 + 20;
        if buf.remaining() < payload_len {
            return Ok(None);
        }

        // protocol string
        let mut prot = [0; 19];
        buf.copy_to_slice(&mut prot);
        // reserved field
        let mut reserved = [0; 8];
        buf.copy_to_slice(&mut reserved);
        // info hash
        let mut info_hash = [0; 20];
        buf.copy_to_slice(&mut info_hash);
        // peer id
        let mut peer_id = [0; 20];
        buf.copy_to_slice(&mut peer_id);

        Ok(Some(Handshake {
            prot,
            reserved,
            info_hash,
            peer_id,
        }))
    }
}

pub struct PeerCodec;

impl Encoder for PeerCodec {
    type Item = Message;
    type Error = io::Error;

    fn encode(
        &mut self,
        msg: Self::Item,
        buf: &mut BytesMut,
    ) -> io::Result<()> {
        use Message::*;
        match msg {
            KeepAlive => {
                // message length prefix
                let msg_len = 0;
                buf.put_u32(msg_len);
                // no payload
            }
            Bitfield(bitfield) => {
                // message length prefix: 1 byte message id and n byte bitfield
                //
                // NOTE: take the length of the underlying storage to get the number
                // of _bytes_, as `bitfield.len()` returns the number of _bits_
                let msg_len = 1 + bitfield.as_slice().len();
                buf.put_u32(msg_len as u32);
                // message id
                buf.put_u8(MessageId::Bitfield as u8);
                // payload
                buf.extend_from_slice(bitfield.as_slice());
            }
            Choke => {
                // message length prefix: 1 byte message id
                let msg_len = 1;
                buf.put_u32(msg_len);
                // message id
                buf.put_u8(MessageId::Choke as u8);
                // no payload
            }
            Unchoke => {
                // message length prefix: 1 byte message id
                let msg_len = 1;
                buf.put_u32(msg_len);
                // message id
                buf.put_u8(MessageId::Unchoke as u8);
                // no payload
            }
            Interested => {
                // message length prefix: 1 byte message id
                let msg_len = 1;
                buf.put_u32(msg_len);
                // message id
                buf.put_u8(MessageId::Interested as u8);
                // no payload
            }
            NotInterested => {
                // message length prefix: 1 byte message id
                let msg_len = 1;
                buf.put_u32(msg_len);
                // message id
                buf.put_u8(MessageId::NotInterested as u8);
                // no payload
            }
            Have { piece_index } => {
                // message length prefix:
                // 1 byte message id and 4 byte piece index
                let msg_len = 1 + 4;
                buf.put_u32(msg_len);
                // message id
                buf.put_u8(MessageId::Have as u8);
                // payload
                buf.put_u32(piece_index);
            }
            Request {
                piece_index,
                offset,
                length,
            } => {
                // message length prefix:
                // 1 byte message id, 4 byte piece index, 4 byte offset, 4 byte
                // length
                let msg_len = 1 + 4 + 4 + 4;
                buf.put_u32(msg_len);
                // message id
                buf.put_u8(MessageId::Request as u8);
                // payload
                buf.put_u32(piece_index);
                buf.put_u32(offset);
                buf.put_u32(length);
            }
            Block {
                piece_index,
                offset,
                block,
            } => {
                // message length prefix:
                // 1 byte message id, 4 byte piece index, 4 byte offset, and n byte
                // block
                let msg_len = 1 + 4 + 4 + block.len() as u32;
                buf.put_u32(msg_len);
                // message id
                buf.put_u8(MessageId::Block as u8);
                // payload
                buf.put_u32(piece_index);
                buf.put_u32(offset);
                buf.put(&block[..]);
            }
            Cancel {
                piece_index,
                offset,
                length,
            } => {
                // message length prefix:
                // 1 byte message id, 4 byte piece index, 4 byte offset, 4 byte
                // length
                let msg_len = 1 + 4 + 4 + 4;
                buf.put_u32(msg_len);
                // message id
                buf.put_u8(MessageId::Cancel as u8);
                // payload
                buf.put_u32(piece_index);
                buf.put_u32(offset);
                buf.put_u32(length);
            }
        }

        Ok(())
    }
}

impl Decoder for PeerCodec {
    type Item = Message;
    type Error = io::Error;

    fn decode(&mut self, buf: &mut BytesMut) -> io::Result<Option<Self::Item>> {
        // the message length header must be present at the minimum, otherwise
        // we can't determine the message type
        if buf.len() < 4 {
            return Ok(None);
        }

        let msg_len = buf.get_u32() as usize;

        // the message length is only 0 if this is a keep alive message (all
        // other message types have at least one more field, the message id)
        if msg_len == 0 {
            return Ok(Some(Message::KeepAlive));
        }

        // check that we got the full payload in the buffer
        if buf.remaining() < msg_len {
            return Ok(None);
        }

        let msg_id = MessageId::try_from(buf.get_u8())?;
        let msg = match msg_id {
            MessageId::Choke => Message::Choke,
            MessageId::Unchoke => Message::Unchoke,
            MessageId::Interested => Message::Interested,
            MessageId::NotInterested => Message::NotInterested,
            MessageId::Have => {
                let piece_index = buf.get_u32();
                Message::Have { piece_index }
            }
            MessageId::Bitfield => {
                // preallocate buffer to the length of bitfield, by subtracting
                // the id from the message length
                let mut bitfield = vec![0; msg_len - 1];
                buf.copy_to_slice(&mut bitfield);
                Message::Bitfield(Bitfield::from_vec(bitfield))
            }
            MessageId::Request => {
                let piece_index = buf.get_u32();
                let offset = buf.get_u32();
                let length = buf.get_u32();
                Message::Request {
                    piece_index,
                    offset,
                    length,
                }
            }
            MessageId::Block => {
                let piece_index = buf.get_u32();
                let offset = buf.get_u32();
                // TODO: here we would want to copy into a pre-allocated buffer
                // rather than create a new buffer, created outside the message
                // codec
                debug_assert!(msg_len > 8);
                // preallocate the vector to the block length, by subtracting
                // the id, piece index and offset lengths from the message
                // length
                let mut block = vec![0; msg_len - 9];
                buf.copy_to_slice(&mut block);
                Message::Block {
                    piece_index,
                    offset,
                    block,
                }
            }
            MessageId::Cancel => {
                let piece_index = buf.get_u32();
                let offset = buf.get_u32();
                let length = buf.get_u32();
                Message::Cancel {
                    piece_index,
                    offset,
                    length,
                }
            }
        };

        Ok(Some(msg))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    // Tests a stream of arbitrary messages to ensure that not only do they
    // encode and then decode correctly (like the individual test cases
    // ascertain), but that the buffer cursor is properly advanced by the codec
    // implementation in both cases.
    #[test]
    fn test_message_stream() {
        let (handshake, encoded_handshake) = make_handshake();
        let msgs = [
            make_choke(),
            make_unchoke(),
            make_interested(),
            make_not_interested(),
            make_bitfield(),
            make_have(),
            make_request(),
            make_block(),
            make_interested(),
            make_cancel(),
            make_block(),
            make_not_interested(),
            make_choke(),
            make_choke(),
        ];

        // create byte stream of all above messages
        let msgs_len =
            msgs.iter().fold(0, |acc, (_, encoded)| acc + encoded.len());
        let mut buf = BytesMut::with_capacity(msgs_len);
        buf.extend_from_slice(&encoded_handshake);
        for (_, encoded) in &msgs {
            buf.extend_from_slice(&encoded);
        }

        // decode messages one by one from the byte stream in the same order as
        // they were encoded, starting with the handshake
        let decoded_handshake = HandshakeCodec.decode(&mut buf).unwrap();
        assert_eq!(decoded_handshake, Some(handshake));
        for (msg, _) in &msgs {
            let decoded_msg = PeerCodec.decode(&mut buf).unwrap();
            assert_eq!(decoded_msg.unwrap(), *msg);
        }
    }

    // Tests the encoding and subsequent decoding of a valid handshake.
    #[test]
    fn test_handshake_codec() {
        let (handshake, expected_encoded) = make_handshake();

        // encode handshake
        let mut encoded = BytesMut::with_capacity(expected_encoded.len());
        HandshakeCodec.encode(handshake, &mut encoded).unwrap();
        assert_eq!(encoded, expected_encoded);

        // don't decode handshake if there aren't enough bytes in source buffer
        let mut partial_encoded = encoded[0..30].into();
        let decoded = HandshakeCodec.decode(&mut partial_encoded).unwrap();
        assert_eq!(decoded, None);

        // decode same handshake
        let decoded = HandshakeCodec.decode(&mut encoded).unwrap();
        assert_eq!(decoded, Some(handshake));
    }

    // Tests that the decoding of various invalid handshake messages results in
    // an error.
    #[test]
    fn test_invalid_handshake_decoding() {
        // try to decode a handshake with an invalid protocol string
        let mut invalid_encoded = {
            let prot = "not the BitTorrent protocol";
            // these buffer values don't matter here as we're only expecting
            // invalid encodings
            let reserved = [0; 8];
            let info_hash = [0; 20];
            let peer_id = [0; 20];

            let buf_len = prot.len() + 49;
            let mut buf = BytesMut::with_capacity(buf_len);
            // the message length prefix is not actually included in the value
            let prot_len = prot.len() as u8;
            buf.put_u8(prot_len);
            buf.extend_from_slice(prot.as_bytes());
            buf.extend_from_slice(&reserved);
            buf.extend_from_slice(&info_hash);
            buf.extend_from_slice(&peer_id);
            buf
        };
        let result = HandshakeCodec.decode(&mut invalid_encoded);
        assert!(result.is_err());
    }

    // Returns a `Handshake` and its expected encoded variant.
    fn make_handshake() -> (Handshake, Bytes) {
        // protocol string
        let mut prot = [0; 19];
        prot.copy_from_slice(PROTOCOL_STRING.as_bytes());

        // the reserved field is all zeros for now as we don't use extensions
        // yet so we're not testing it
        let reserved = [0; 8];

        // this is not a valid info hash but it doesn't matter for the purposes
        // of this test
        const INFO_HASH: &str = "da39a3ee5e6b4b0d3255";
        let mut info_hash = [0; 20];
        info_hash.copy_from_slice(INFO_HASH.as_bytes());

        const PEER_ID: &str = "cbt-2020-03-03-00000";
        let mut peer_id = [0; 20];
        peer_id.copy_from_slice(PEER_ID.as_bytes());

        let handshake = Handshake {
            prot,
            reserved,
            info_hash,
            peer_id,
        };

        // TODO: consider using hard coded bye array for expected value rather
        // than building up result (as that's what the encoder does too and we
        // need to test that it does it correctly)
        let encoded = {
            let buf_len = 68;
            let mut buf = Vec::with_capacity(buf_len);
            // the message length prefix is not actually included in the value
            let prot_len = prot.len() as u8;
            buf.push(prot_len);
            buf.extend_from_slice(&prot);
            buf.extend_from_slice(&reserved);
            buf.extend_from_slice(&info_hash);
            buf.extend_from_slice(&peer_id);
            buf
        };

        (handshake, encoded.into())
    }

    // Tests the encoding and subsequent decoding of a valid 'choke' message.
    #[test]
    fn test_choke_codec() {
        let (msg, expected_encoded) = make_choke();
        assert_message_codec(msg, expected_encoded);
    }

    // Tests the encoding and subsequent decoding of a valid 'unchoke' message.
    #[test]
    fn test_unchoke_codec() {
        let (msg, expected_encoded) = make_unchoke();
        assert_message_codec(msg, expected_encoded);
    }

    // Tests the encoding and subsequent decoding of a valid 'interested'
    // message.
    #[test]
    fn test_interested_codec() {
        let (msg, expected_encoded) = make_interested();
        assert_message_codec(msg, expected_encoded);
    }

    // Tests the encoding and subsequent decoding of a valid 'not interested'
    // message.
    #[test]
    fn test_not_interested_codec() {
        let (msg, expected_encoded) = make_not_interested();
        assert_message_codec(msg, expected_encoded);
    }

    // Tests the encoding and subsequent decoding of a valid 'bitfield' message.
    #[test]
    fn test_bitfield_codec() {
        let (msg, expected_encoded) = make_bitfield();
        assert_message_codec(msg, expected_encoded);
    }

    // Tests the encoding and subsequent decoding of a valid 'have' message.
    #[test]
    fn test_have_codec() {
        let (msg, expected_encoded) = make_have();
        assert_message_codec(msg, expected_encoded);
    }

    // Tests the encoding and subsequent decoding of a valid 'request' message.
    #[test]
    fn test_request_codec() {
        let (msg, expected_encoded) = make_request();
        assert_message_codec(msg, expected_encoded);
    }

    // Tests the encoding and subsequent decoding of a valid 'block' message.
    #[test]
    fn test_block_codec() {
        let (msg, expected_encoded) = make_block();
        assert_message_codec(msg, expected_encoded);
    }

    // Tests the encoding and subsequent decoding of a valid 'cancel' message.
    #[test]
    fn test_cancel_codec() {
        let (msg, expected_encoded) = make_cancel();
        assert_message_codec(msg, expected_encoded);
    }

    // Helper function that asserts that a message is encoded and subsequently
    // decoded correctly.
    fn assert_message_codec(msg: Message, expected_encoded: Bytes) {
        // encode message
        let mut encoded = BytesMut::with_capacity(expected_encoded.len());
        PeerCodec.encode(msg.clone(), &mut encoded).unwrap();
        assert_eq!(encoded, expected_encoded);

        // don't decode message if there aren't enough bytes in source buffer
        let mut partial_encoded = encoded[0..encoded.len() - 1].into();
        let decoded = PeerCodec.decode(&mut partial_encoded).unwrap();
        assert_eq!(decoded, None);

        // decode same message
        let decoded = PeerCodec.decode(&mut encoded).unwrap();
        assert_eq!(decoded, Some(msg));
    }

    // Returns `Choke` and its expected encoded variant.
    fn make_choke() -> (Message, Bytes) {
        (
            Message::Choke,
            make_empty_msg_encoded_payload(MessageId::Choke),
        )
    }

    // Returns `Unchoke` and its expected encoded variant.
    fn make_unchoke() -> (Message, Bytes) {
        (
            Message::Unchoke,
            make_empty_msg_encoded_payload(MessageId::Unchoke),
        )
    }

    // Returns `Interested` and its expected encoded variant.
    fn make_interested() -> (Message, Bytes) {
        (
            Message::Interested,
            make_empty_msg_encoded_payload(MessageId::Interested),
        )
    }

    // Returns `NotInterested` and its expected encoded variant.
    fn make_not_interested() -> (Message, Bytes) {
        (
            Message::NotInterested,
            make_empty_msg_encoded_payload(MessageId::NotInterested),
        )
    }

    // Helper used to create 'choke', 'unchoke', 'interested', and 'not
    // interested' encoded messages that all have the same format.
    fn make_empty_msg_encoded_payload(id: MessageId) -> Bytes {
        // 1 byte message id
        let msg_len = 1;
        // 4 byte message length prefix and message length
        let buf_len = 4 + msg_len as usize;
        let mut buf = BytesMut::with_capacity(buf_len);
        buf.put_u32(msg_len);
        buf.put_u8(id as u8);
        buf.into()
    }

    // Returns `Bitfield` and its expected encoded variant.
    fn make_bitfield() -> (Message, Bytes) {
        let bitfield =
            Bitfield::from_slice(&[0b11001001, 0b10000011, 0b11111011]);
        let encoded = {
            // 1 byte message id and n byte f bitfield
            //
            // NOTE: take the length of the underlying storage to get the number
            // of _bytes_, as `bitfield.len()` returns the number of _bits_
            let msg_len = 1 + bitfield.as_slice().len();
            // 4 byte message length prefix and message length
            let buf_len = 4 + msg_len;
            let mut buf = BytesMut::with_capacity(buf_len);
            buf.put_u32(msg_len as u32);
            buf.put_u8(MessageId::Bitfield as u8);
            buf.extend_from_slice(bitfield.as_slice());
            buf
        };
        let msg = Message::Bitfield(bitfield);
        (msg, encoded.into())
    }

    // Returns `Have` and its expected encoded variant.
    fn make_have() -> (Message, Bytes) {
        let piece_index = 42;
        let msg = Message::Have { piece_index };
        let encoded = {
            // 1 byte message id and 4 byte piece index
            let msg_len = 1 + 4;
            // 4 byte message length prefix and message length
            let buf_len = 4 + msg_len;
            let mut buf = BytesMut::with_capacity(buf_len);
            buf.put_u32(msg_len as u32);
            buf.put_u8(MessageId::Have as u8);
            buf.put_u32(piece_index);
            buf
        };
        (msg, encoded.into())
    }

    // Returns `Request` and its expected encoded variant.
    fn make_request() -> (Message, Bytes) {
        let piece_index = 42;
        let offset = 0x4000;
        let length = 4 * 0x4000;
        let msg = Message::Request {
            piece_index,
            offset,
            length,
        };
        let encoded = make_block_info_encoded_msg_payload(
            MessageId::Request,
            piece_index,
            offset,
            length,
        );
        (msg, encoded)
    }

    // Returns `Block` and its expected encoded variant.
    fn make_block() -> (Message, Bytes) {
        let piece_index = 42;
        let offset = 0x4000;
        let block = vec![0; 4 * 0x4000];
        // TODO: fill the block with random values
        let encoded = {
            // 1 byte message id, 4 byte piece index, 4 byte offset, and n byte
            // block
            let msg_len = 1 + 4 + 4 + block.len();
            // 4 byte message length prefix and message length
            let buf_len = 4 + msg_len;
            let mut buf = BytesMut::with_capacity(buf_len);
            buf.put_u32(msg_len as u32);
            buf.put_u8(MessageId::Block as u8);
            buf.put_u32(piece_index);
            buf.put_u32(offset);
            buf.extend_from_slice(&block);
            buf
        };
        let msg = Message::Block {
            piece_index,
            offset,
            block,
        };
        (msg, encoded.into())
    }

    // Returns `Cancel` and its expected encoded variant.
    fn make_cancel() -> (Message, Bytes) {
        let piece_index = 42;
        let offset = 0x4000;
        let length = 4 * 0x4000;
        let msg = Message::Cancel {
            piece_index,
            offset,
            length,
        };
        let encoded = make_block_info_encoded_msg_payload(
            MessageId::Cancel,
            piece_index,
            offset,
            length,
        );
        (msg, encoded)
    }

    // Helper used to create 'request' and 'cancel' encoded messages that have
    // the same format.
    fn make_block_info_encoded_msg_payload(
        id: MessageId,
        piece_index: u32,
        offset: u32,
        length: u32,
    ) -> Bytes {
        // 1 byte message id, 4 byte piece index, 4 byte offset, 4 byte
        // length
        let msg_len = 1 + 4 + 4 + 4;
        // 4 byte message length prefix and message length
        let buf_len = 4 + msg_len as usize;
        let mut buf = BytesMut::with_capacity(buf_len);
        buf.put_u32(msg_len);
        buf.put_u8(id as u8);
        buf.put_u32(piece_index);
        buf.put_u32(offset);
        buf.put_u32(length);
        buf.into()
    }
}
