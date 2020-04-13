# Cratetorrent design

This document contains notes on the *current* design of the implementation of
both the cratetorrent library as well as the CLI cratetorrent binary. It serves
as a high-level documentation of the code and documenting design decisions that
may not be self-evident. It is continuously expanded as more features are added.
It is important to note that this document only reflects the current state of
the code and does not include future features, unless otherwise noted.

The library can be found in the [`cratetorrent`](./cratetorrent) folder, while
the binary is in [`cratetorrent-cli`](./cratetorrent-cli). The rationale for
developing a library and a binary simultaneously is because big emphasis is
placed on the usability of the cratetorrent library API, and testing the API, as
it is developed, in a near real world scenario helps to integrate feedback
immediately.


## Sources

- The official protocol specification: http://bittorrent.org/beps/bep_0003.html
- My previous torrent engine implementation (in C++), Tide: (https://github.com/mandreyel/tide)
- The libtorrent blog: https://blog.libtorrent.org/

**NOTE**: This document assumes familiarity with the official protocol.


## Overview

Currently the "engine" is simplified to perform only a download of a single file
into memory via a single connection. Still, such a simple goal requires quite a
few components (each detailed in its own section):

- [metainfo](#metainfo), which contains necessary information to start the
  torrent;
- [engine](#engine), which orchestrates all components of the torrent engine,
- [torrent](#torrent), which coordinates the download of a torrent;
- a [piece picker](#piece-picker) per torrent, that selects the next piece to
  download;
- [peer connection](#peer-connection) in a torrent (currently only a single
  downloader), which implement the peer protocol and perform the actual
  download;
- in progress [piece download](#piece-download), which track the state of an
  ongoing piece download;
- [disk IO](#disk-io), which saves downloaded file pieces.

At the end of the document, there is an [overview](#anatomy-of-a-block-fetch) of
a block fetch, incorporating the above key components.

The binary takes as command line arguments the single seed IP address and port
pair, the torrent metainfo, and sets up a single torrent, piece picker, and a single peer
connection to that seed. All IO is done asynchronously, and the torrent has a
periodic update function, run every second.


## Metainfo

A torrent's metadata is described in the metainfo file. Starting a download or
  upload requires feeding such a metainfo file to the torrent client, which will
  extract relevant information from it about the torrent and start the
  upload/download. The contents of the metainfo file must be UTF-8 encoded.

There is an easy way to parse metainfo files using the [`serde` bencode
extension](https://github.com/toby/serde-bencode).

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

### Torrent info hash

The hash of the torrent is the SHA1 hash of the raw (bencoded) string of the
metainfo's `info` key's value.

Note that this can be problematic as some metainfo files define fields not
supported or used by cratetorrent, and currently the way we generate this hash
is by encoding the parsed metainfo struct again into its raw bencoded
representation, and this way we may not serialize fields originally present in
the source metainfo.
A better approach would be to keep the underlying raw source (as in Tide), but
  this is not currently possible with the current solution of using `serde`.


## Engine

Currently there is no explicit entity, but simply an engine module that provides
a public method to start a torrent.


## Torrent

- Contains the piece picker and a single connection to a seed from which to
  download the file.
- Also contains other metadata relevant to the torrent, such as its info hash,
  the files it needs to download, the destination directory, and others.
- Torrent tick: periodically loops through all its peer connections and performs
  actions like stats collections, later choking/unchoking, resume state saving,
  and others.
- A peer session is spawned on a new
  [task](https://docs.rs/tokio/0.2.13/tokio/task), as otherwise we'd enter all
  sorts of lifetime issues by running the torrent and the peer session in the
  same task's async loop. More research on this and alternatives needs to be
  done.
- Communication with peer sessions would happen through `tokio`'s `mpsc` queues,
  since a peer session is spawned on a task which may be run by a different
  thread on which torrent is executing


## Piece picker

Each torrent has a piece picker, which is the entity that collects information
about the torrent swarm's piece availability in order to make a more optimal
decision on what piece to pick next. For now though, pieces are downloaded
sequentially and more complex algorithms will be implemented later (see research
notes).

The piece picker holds a vector pre-allocated to the number of pieces in the
torrent and each element in this vector contains metadata about the piece:
whether we have it or not and its frequency in the swarm.

Later the internal data structures of the piece picker will most likely be
changed to be more optimal for the rarest-pieces-first algorithm (the default
defined by the standard).

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
    outstanding until choked or the download is complete. See the [download
    pipeline section](#download-pipeline).
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
- All IO is asynchronous, and the "session loop" (that runs the peer session) is
  going to multiplex several event sources (futures): receiving of messages from
  peer, saving of a block to disk, updates from owning torrent.

### Startup

1. Connect to TCP socket of peer.
2. We're in the handshake exchange state.
3. If this is an outbound connection, start by sending a handshake, otherwise
   just start receiving and wait for incoming handshake.
4. Receive handshake, and if it is valid and the torrent info hash checks out,
   send our own handshake.
5. The connected peers may now optionally exchange their piece availability.
6. After this step, peers start exchanging normal messages.

### Download pipeline

After receiving an unchoke message from our seed, we can start making requests.
In order to download blocks in the fastest way, we always need to keep the
number of requests outstanding that saturates the link to the peer we're
downloading from.

From a relevant [libtorrent
blog post](https://blog.libtorrent.org/2011/11/requesting-pieces/):

> Deciding how many outstanding requests to keep to peers typically is based on
  the bandwidth delay product, or a simplified model thereof.

> The bandwidth delay product is the bandwidth capacity of a link multiplied by
  the latency of the link. It represents the number of bytes that fit in the
  wire and buffers along the path. In order to fully utilize a link, one needs
  to keep at least the bandwidth delay product worth of bytes outstanding
  requests at any given time. If fewer bytes than that are kept in outstanding
  requests, the other end will satisfy all the requests and then have to wait
  (wasting bandwidth) until it receives more requests.

> If many more requests than the bandwidth delay product is kept outstanding,
  more blocks than necessary will be marked as requested, and waste potential to
  request them from other peers, completing a piece sooner. Typically, keeping
  too few outstanding requests has a much more severe impact on performance than
  keeping too many.

> On machines running at max disk capacity, the actual disk-job that a request
  result in may take longer to be satisfied by the drive than the network
  latency. It’s important to take the disk latency into account, as to not under
  estimate the latency of the link. i.e. the link could be considered to go all
  the way down to the disk [...] This is another reason to rather
  over-estimate the latency of the network connection than to under-estimate it.

> A simple way to determine the number of outstanding block requests to keep to
  a peer is to take the current download rate (in bytes per second) we see from
  that peer and divide it by the block size (16 kiB). That assumes the latency
  of the link to be 1 second.
> num_pending = download_rate / 0x4000;
> It might make sense to increase the assumed latency to 1.5 or 2 seconds, in
  order to take into account the case where the other ends disk latency is the
  dominant factor. Keep in mind that over-estimating is cheaper than
  under-estimating.

Based on the above, each peer session needs to maintain an "optimal request
queue size" value (approximately the bandwidth-delay product), which is the
number of block requests it keeps outstanding to fully saturate the link.

This value is derived by collecting a running average of the downloaded bytes
per second, as well as the average request latency, to arrive at the
bandwidth-delay product B x D. This value is recalculated every time we receive
a block, in order to always keep the link fully saturated. See
[wikipedia](https://en.wikipedia.org/wiki/Bandwidth-delay_product) for more.

To emphasize the value of this optimization, let's see a visual example of
comparing two connections each downloading two blocks on a link with the same
capacity, where one pair of peers' link is not kept saturated and another pair
of peers whose is:
```
Legend
------
R: request
B: block
{A,B,C,D}: peers

  A      B        C      D
R |      |      R |      |
  |\     |        |\     |
  | \    |      R | \    |
  |  \   |        |\ \   |
  |   \  |        | \ \  |
  |    \ |        |  \ \ |
  |     \|        |   \ \|
  |      | B      |    \ | B
  |     /|        |     /|
  |    / |        |    / | B
  |   /  |        |   / /|
  |  /   |        |  / / |
  | /    |        | / /  |
  |/     |        |/ /   |
R |      |        | /    |
  |\     |        |/     |
  | \    |
  |  \   |
  |   \  |
  |    \ |
  |     \|
  |      | B
  |     /|
  |    / |
  |   /  |
  |  /   |
  | /    |
  |/     |
```

And even from such a simple example as this, it can be concluded as a feature and not
premature optimization.

What is needed for this to work reliably is timing out requests, but as of this
writing that is not yet implemented.

For now, though, the download pipeline is set to a fixed size of 4, but this is
to be added as a separate step under the optimization milestone.

### Session algorithm

A simplified version of the peer session algorithm follows.

1. Connect to peer and send the BitTorrent handshake.
2. Receive and verify peer's handshake.
3. Start receiving messages.
4. Receive peer's bitfield. If the peer didn't send a bitfield, or it doesn't
   contain all pieces (i.e.  peer is not a seed), we abort the connection as
   only downloading from seed's are supported.
5. Tell peer we're interested.
5. Wait for peer to unchoke us.
6. From this point on we can start making block requests:
   1. Fill the request pipeline with the optimal number of requests for blocks
      from existing and/or new piece downloads.
   2. Wait for requested blocks.
   3. Mark requested blocks as received with their corresponding piece download.
   4. If the piece is complete, mark it as finished with the piece picker.
   5. After each complete piece, check that we have all pieces. If so, conclude
      the download, otherwise make more requests and restart.

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
most significant bit to least significant bit, respectively, byte 1 corresponds
to indices 8-15, and so on. E.g. given the first byte `0b1100'0001` in the
bitfield means we have pieces 0, 1, and 7.

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

A piece download tracks the piece completion of an ongoing piece download. It
  will play an important role in optimizing download performance, but none of
  that is implemented for now (see research notes).


## Disk IO

This entity (called `Disk` in the code) is responsible for everything disk
storage related. This includes:
- allocating a torrent's files at download start;
- hashing downloaded file pieces and saving them to disk;
- once seeding functionality is implemented: reading blocks from disk.

### High level architecture

All disk related activity is spawned on a separate task by the torrent engine,
and communication with it happens through `mpsc` channels. The alternative is
for `Disk` to not be on its own task but simply be referenced (through an `Arc`)
by all entities using it. In this case, we would have the following issues:
- While disk IO itself is spawned on blocking tasks (see below), we need to
  await its result. This means that for e.g. block writes initiated from a peer
  session task, the peer session task would be blocked (since await calls in the
  same `async` block are sequential). This is the most crucial one.
- Increased complexity due to lifetime and synchronization issues with multiple
  peer session tasks referring to the same `Disk`, whereas with the task based
  solution `Disk` is the only one accessing its internal state.
- Worse separation of concerns, leading to code that is more difficult to reason
  about and thus potentially more bugs. Whereas having `Disk` on a separate task
  allows for a sequential model of execution, which is clearer and less
  error-prone.

Spawning the disk task returns a command channel (aka disk handle) and a global
alert port (aka receiver): the former is used for sending commands to disk and
the latter is used to receive back the results of certain commands. However,
because `mpsc` channels can only have a single receiver and because each torrent
needs its own alert port, we a second layer of alert channels for torrents.

Thus, when `Disk` receives a command to create a torrent, it creates the torrent
specific alert channel's send and receive halves, and the latter is returned in
the torrent allocation command result via the global alert channel, so engine
can hand it down to the new torrent. A step by step example follows.

#### Example workflow
1. `Engine` creates a new torrent, and part of this routine allocates the torrent
   on disk via the disk command channel.
2. `Disk` creates a `Torrent` in-memory entry and allocates torrent and creates a
   new alert channel pair and sends the receiving half via the global alert
   port.
3. `Engine` listens on the alert port for the allocation result containing the
   torrent alert port (or an error).
4. `Engine` creates a `Torrent`, which spawns `PeerSession`s with a copy of the disk
   handle, and starts downloading blocks.
5. Each peer session saves blocks to disk by sending block write commands to
   `Disk` via the handle.
6. Blocks are usually buffered until their piece is complete, at which point the
   blocks are written to disk. The result of the disk write is advertised to
   torrent via its alert channel.
7. Torrent receives and handles the block write result.

Caveat emptor: the performance implications of this indirection needs to be
tested, `mpsc` is not guaranteed to be the good choice here.

To summarize, sending alerts from the disk task is two-tiered:
- global (e.g. the result of allocating a new torrent),
- per torrent (e.g. the result of writing blocks to disk).

### Responsibilities

#### Torrent file allocation
Currently only a single file download is supported, so the file allocation is
very simple: the to-be-downloaded file is checked for existence and the first
write creates the file.
Late we will want to pre-allocate sparse files truncated to the download length.

#### Saving to disk
The primary purpose of `Disk` right now is to buffer downloaded blocks in memory
until the piece is complete, hash the piece, notify torrent of the result, and,
if the resulting hash matches the expected one, save to disk.

Efficiency is key here; a torrent application is twofold IO bound: on disk
and on the network. To achieve the fastest download given a fixed network
bandwidth is to buffer the whole download in memory and write a file to disk in
one go, i.e. to not produce disk backpressure for the download.

However, this is not practical in most cases. For one, the download host may be
shut down mid-download, which would lead to the loss of the existing downloaded
data. The other reason is the desire to constrain memory usage as the user may
not have enough RAM to buffer an entire download, or even if they do, they would
likely wish that other programs running in parallel would also have sufficient
RAM.

For these reasons cratetorrent will be highly configurable to serve all needs,
but will aim to provide sane, good enough defaults for most use cases. For now,
though, the implementation is very simple: we download sequentially, and buffer
blocks of a piece until the piece is complete, after which that piece is flushed
to disk. The rationale for saving a piece to disk as soon as it is complete is
twofold:
- being defensive: so that in an event of shutdown, as much data is persisted as
  possible;
- simplicity of the implementation of the MVP. (Later it will be possible to
  configure cratetorrent to buffer several pieces before flushing them to disk,
  or buffer at most `n` blocks, which may be fewer than is in a torrent's
  piece.)

##### Vectored IO
A peer downloads pieces in 4 KiB blocks and to save overhead of concatenating
these buffers these blocks are stored in the disk write buffer as is, i.e the
write buffer is a vector of byte vectors.
Writing each block to disk as a separate system call would incur tremendous
overhead (relative to the other operations in most of cratetorrent), especially
that context switches into kernel space have become far more expensive lately to
mitigate risks of speculative execution, so these buffers are flushed to disk in
one system call, using [vectored
IO](http://man7.org/linux/man-pages/man2/readv.2.html).

While in most of the code asynchrony is provided for by `tokio`, all disk IO is
synchronous because `tokio_fs`'s vectored write implementation is just a wrapper
around repeatedly calling `write` for each buffer, defeating any performance
gains. For these instance of blocking IO, we make use of `tokio`'s builtin
threadpool and spawn the blocking IO
[tasks](https://docs.rs/tokio/0.2.17/tokio/task/fn.spawn_blocking.html) on it.

For now `writev` is used, as that's the API that the standard lib `File`
[exposes](https://doc.rust-lang.org/std/io/trait.Write.html#method.write_vectored),
however, this requires that the file cursor is at the correct position, and
requires seeking to the correct position if it's not. Since this needlessly
adds another context switch, but ideally
[`pwritev`](https://docs.rs/nix/0.17.0/nix/sys/uio/fn.pwritev.html) would be
used (where the position of the write is specified as part of the write
operation). For now the standard lib is used for simplicity as this is not yet
an issue due to downloading sequentially, but this will be changed later.


## Anatomy of a block fetch

1. `PeerSession` requests `n` blocks from peer.
2. For each received block, `DiskHandle::write_block` is called which sends a
   message via the command channel to instruct `Disk` to place the block in the
   write buffer.
3. Once the corresponding piece has all blocks in it, it is hashed and, if the
   hash matches the expected hash, saved to disk.
4. The written blocks and the hash result is communicated back to torrent via a
   channel (or the IO error if any occurred).
5. `Torrent` receives this message, processes it, and forwards it to the peer
   session.
