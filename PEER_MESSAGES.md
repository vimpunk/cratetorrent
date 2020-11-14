# Peer messages

This section contains a description of the format and context of the  message
types that the base protocol defines.

## Message length
All messages start with a message length prefix, which contains the _payload's_
message length, that is, it does _not_ contain the length of the length header.
Except for the initial handshake, whose message length field is a single byte,
all message's message lengths are 4 byte big-endian integers.

## Message ID
Except for the initial handshake, each message's 4 byte length prefix must be
followed by a 1 byte message id. The id of the message is an unsigned integer
(which is included in the message subheaders further below).

## Legend
The format of describing a message in the document below is as follows: every
message may contain one or more "fields", such as:
```
<bytes: name=value>
```
where:
- `<`, `>`, `:`, and `=` are the symbols used solely in this documentation,
  but are *not transmitted*,
- **bytes**: is the length of the message field in bytes,
- **name**: is the name we use to refer to the message field in the code (but
  the value is *not transmitted*);
- **value**: is the value of the field, which *is* transmitted.

## **`handshake`**
```
<1: prot len=19>
<19: prot="BitTorrent protocol">
<8: reserved>
<20: info hash>
<20: peer id>
```
This is the very first message exchanged. If the peer's protocol string (`BitTorrent
protocol`) or the info hash differs from ours, the connection is severed. The
reserved field is 8 zero bytes, but will later be used to set which extensions
the peer supports. The peer id is usually the client name and version.

## **`keep alive`**
```
<4: len=0>
```

## **`[5] bitfield`**
```
<4: len=1+X>
<1: id=5>
<X: bitfield>
```

Only ever sent as the first message after the handshake. The payload of this
message is a bitfield whose indices represent the file pieces in the torrent and
is used to tell the other peer which pieces the sender has available (each
available piece's bitfield value is 1). Byte 0 corresponds to indices 0-7, from
most significant bit to least significant bit, respectively, byte 1 corresponds
to indices 8-15, and so on. E.g. given the first byte `0b1100'0001` in the
bitfield means we have pieces 0, 1, and 7.

If a peer doesn't have any pieces downloaded, they need not send
this message.

## **`[0] choke`**
```
<4: len=1>
<1: id=0>
```

Choke the peer, letting them know that the may _not_ download any pieces.

This is not currently used as seeding functionality is not implemented.

## **`[1] unchoke`**
```
<4: len=1>
<1: id=1>
```

Unchoke the peer, letting them know that the may download.

This is not currently used as seeding functionality is not implemented.

## **`[2] interested`**
```
<4: len=1>
<1: id=2>
```

Let the peer know that we are interested in the pieces that it has
available.

## **`[3] not interested`**
```
<4: len=1>
<1: id=3>
```

Let the peer know that we are _not_ interested in the pieces that it has
available because we also have those pieces.

## **`[4] have`**
```
<4: len=5>
<1: id=6>
<4: piece index>
```

This messages is sent when the peer wishes to announce that they downloaded a
new piece. This is only sent if the piece's hash verification checked out.

## **`[6] request`**
```
<4: len=13>
<1: id=6>
<4: piece index>
<4: offset>
<4: length=16384>
```

This message is sent when a downloader requests a chunk of a file piece from its
peer. It specifies the piece index, the offset into that piece, and the length
of the block. As noted above, due to nearly all clients in the wild reject
requests that are not 16 KiB, we can assume the length field to always be 16
KiB.

Whether we should also reject requests for different values is an
open-ended question, as only allowing 16 KiB blocks allows for certain
optimizations.

## **`[7] piece`** (also referred to as **`block`**)
```
<4: len=9+X>
<1: id=7>
<4: piece index>
<4: offset>
<X: block>
```

`piece` messages are the responses to `request` messages, containing the request
block's payload. It is possible for an unexpected piece to arrive if choke and
unchoke messages are sent in quick succession, if transfer is going slowly, or
both.

**Note** that in the protocol the term `piece` is used to describe the pieces
into which a torrent download is split up, as well as the download
request/response messages. For clarity, the term `block` is used for piece
(block) download messages throughout the code and the rest of the document,
while `piece` is only used for file pieces.

## **`[8] cancel`**
```
<4: len=13>
<1: id=8>
<4: piece index>
<4: offset>
<4: length=16384>
```

Used to cancel an outstanding download request. Generally used towards the end
of a download in `endgame mode`.

When a download is almost complete, there's a tendency for the last few pieces
to all be downloaded off a single hosed modem line, taking a very long time. To
make sure the last few pieces come in quickly, once requests for all pieces a
given downloader doesn't have yet are currently pending, it sends requests for
everything to everyone it's downloading from. To keep this from becoming
horribly inefficient, it sends cancels to everyone else every time a piece
arrives.
