# cratetorrent design doc

This document contains notes on the *current* design of the implementation of
cratetorrent. It serves as a high-level documentation of the code and
documenting design decisions that may not be self-evident. It is continuously
expanded as more features are added. It is important to note that this document
only reflects the current state of the code and does not include future
features, unless otherwise noted.


## Sources

- The official protocol specification: http://bittorrent.org/beps/bep_0003.html
- My previous torrent engine implementation (in C++): https://github.com/mandreyel/tide
- The libtorrent blog: https://blog.libtorrent.org/


## Overview

Currently the "engine" is simplified to perform only a download of a single
file for a single connection. Still, such a simple goal requires quite a few
components (each detailed in its own section):

- [metainfo](#metainfo), which contains torrent information;
- [disk IO](#disk-io), which saves downloaded file pieces;
- [torrents](#torrent), which coordinates the download of a torrent;
- a [piece picker](#piece-picker) per torrent, that selects the next piece to
  download;
- [peer connections](#peer-connection) in a torrent (currently only a single
  downloader), which implements the peer protocol and performs the actual
  download;
- in progress [piece downloads](#piece-download), which tracks the state of an
  ongoing piece download.

The binary takes as command line arguments the single seed IP address and port
pair, the torrent metainfo, and sets up a single torrent, piece picker, and a single peer
connection to that seed. All IO is done asynchronously, and the torrent has a
periodic update function, run every second.


## Metainfo

A torrent's metadata is described in the metainfo file. Starting a download or
  upload requires feeding such a metainfo file to the torrent client, which will
  extract relevant information from it about the torrent and start the
  upload/download. The contents of the metainfo file must be UTF-8 encoded.

However, cratetorrent doesn't currently implement this, so the relevant
parameters from the metainfo file are passed as _command line arguments_ to the
test binary.

The metainfo is a dictionary of key values. It has the following two top-level
keys:
- **`announce`**: The URL of the torrent's tracker.
- **`info`**: The torrent information, described below.

### Announce

Trackers are not implemented so this is disregarded for now.

### Info

- **`name`**: UTF-8 encoded string of the name of the torrent, which in the case
  of a single file download is the name of the downloaded file, or in the case
  of a multi-file download the name of the downloaded directory. However, when
  downloading, the user should be able to set a different name.
- **`piece length`**: Files are split up into _pieces_, and the length of a
  piece is the number of bytes in a piece, except for possibly the last piece,
  which may be shorter. This value is usually a power of two, and usually some
  multiple of 4KiB. File pieces are indexed from zero.
- **`pieces`**: A list of SHA1 hashes, that represent the expected value of
  hashing each of the file pieces.
- **`length`** exclusive or **`files`**:
  - **`length`**: In case of a single file download, this is the length of the
    file, and the name of the file is the download name.
  - **`files`**: In case of multiple files, this is a list of files representing
    the directory structure of the torrent archive, and each element is a
    dictionary:
    - **`length`**: The length of the file.
    - **`path`**: The UTF-8 encoded full path of the file, relative to the
      download root.


## Disk IO

To be added.


## Torrent

- The hash of the torrent is the SHA1 hash of the raw (bencoded) string of the
  metainfo's `info` key's value.
- Contains the piece picker and a set of connections (for now only one).
- Torrent tick: periodically loops through all its peer connections and
  performs actions like stats collections, later choking/unchoking, resume
  state saving, and others.

## Piece picker

Each torrent has a piece picker, which is the entity that collects information
about the torrent swarm's piece availability in order to make a more optimal
decision on what piece to pick next. For now though, pieces are downloaded
sequentially and more complex algorithms will be implemented later (see research
notes).


## Peer connection

This represents an open or closed connection to another BitTorrent peer. It
implements the protocol defined in specification, under "peer protocol". The
specification defines connections over TCP or uTP (uTorrent protocol), but
currently only TCP is supported.

- Peer connections are symmetrical.
- Peers request from each other file pieces by their indices.
- Each side of the connection could be in one of two states (from the POV of the
  protocol): choked or unchoked. When a peer is choked, it means that it may not
  request pieces.
- Thus transfer of data only takes place when one side is interested and is not
  choked. Peers need to express interest.
- Connections start out choked and not interested.
- Download pipeline: downloaders should keep several piece requests queued up
  for optimal throughput rates. This is achieved by sending several piece
    requests at a time and always keeping the optimal number of piece requests
    outstanding until choked or the download is complete. However, note that
    this is not implemented at the time, it is only kept here as a note for
    future implementations.
- The first exchanged message _must_ be the handshake.
- Apart from the handshake length prefix, all integers are encoded as four bytes
  in big-endian (i.e. multi-byte integers are sent in network order).
- Each peer sends empty keep-alive messages, but timeouts could occur sooner
  when data is requested but not received in time.
- While a file is split into pieces, peers usually never download the entire
  piece. Rather, downloads happen for _blocks_, which are the equal size units
  within pieces. This is almost always **2^14 bytes**, or **16 KiB**, some
  clients even rejecting to service requests for piece blocks larger than this
  value.
- All IO is asynchronous, and there will be two
[tasks](https://doc.rust-lang.org/std/task/index.html): one for socket read and
one for socket write. This is necessary as sending and receiving messages is a
mostly separate operation.


### Messages

This section contains a description of the format and context of the  message
types that the base protocol defines.

#### Message length
All messages start with a message length prefix, which contains the _payload's_
message length, that is, it does _not_ contain the length of the length header.
Except for the initial handshake, whose message length field is a single byte,
all message's message lengths are 4 byte big-endian integers.

#### Message ID
Except for the initial handshake, each message's 4 byte length prefix must be
followed by a 1 byte message id. The id of the message is an unsigned integer
(which is included in the message subheaders further below).

#### Legend
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
  the value is not actually transmitted);
- **value**: is the value of the field, which is actually transmitted.

The rationale behind this notation is to densely store information so that
when developing message related code, information can quickly
be accessed at a glance.

#### **`handshake`**
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

#### **`keep alive`**
```
<4: len=0>
```

#### **`[5] bitfield`**
```
<4: len=1+X>
<1: id=5>
<X: bitfield>
```

Only ever sent as the first message after the handshake. The payload of this
message is a bitfield whose indices represent the file pieces in the torrent and
is used to tell the other peer which pieces the sender has available (each
available piece's bitfield value is 1). Byte 0 corresponds to indices 0-7, from
high bit to low bit, respectively, byte 1 corresponds to indices 8-15, and so
on. E.g. given the first byte `0b1100'0001` in the bitfield means we have pieces
0, 1, and 7.

If a peer doesn't have any pieces downloaded, they need not send
this message.

#### **`[0] choke`**
```
<4: len=1>
<1: id=0>
```

Choke the peer, letting them know that the may _not_ download any pieces.

This is not currently used as seeding functionality is not implemented.

#### **`[1] unchoke`**
```
<4: len=1>
<1: id=1>
```

Unchoke the peer, letting them know that the may download.

This is not currently used as seeding functionality is not implemented.

#### **`[2] interested`**
```
<4: len=1>
<1: id=2>
```

Let the peer know that we are interested in the pieces that it has
available.

#### **`[3] not interested`**
```
<4: len=1>
<1: id=3>
```

Let the peer know that we are _not_ interested in the pieces that it has
available because we also have those pieces.

#### **`[4] have`**
```
<4: len=5>
<1: id=6>
<4: piece index>
```

This messages is sent when the peer wishes to announce that they downloaded a
new piece. This is only sent if the piece's hash verification checked out.

#### **`[6] request`**
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

#### **`[7] piece`** (also referred to as **`block`**)
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

#### **`[8] cancel`**
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


## Piece download

A piece download tracks the piece completion. It will play an important role in
  optimizing download performance, but none of that is implemented for now (see
  research notes).
