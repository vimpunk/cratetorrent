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


## Documentation

This document is focused on the high-level design of cratetorrent, but the code
has ample of inline documentation (beyond the documentation of public APIs). To
view it, you need to run the following command from the repo root:
```bash
cargo doc --document-private-items --workspace --exclude cratetorrent-cli --open
```


## Sources

- The official protocol specification: http://bittorrent.org/beps/bep_0003.html
- My previous torrent engine implementation (in C++), Tide: (https://github.com/mandreyel/tide)
- The libtorrent blog: https://blog.libtorrent.org/

**NOTE**: This document assumes familiarity with the official protocol.


## Overview

Currently the "engine" is simplified to perform a download from seed-only
connections. Still, such a simple goal requires quite a few components (each
detailed in its own section):

- [metainfo](#metainfo), which contains necessary information to start the
  torrent;
- [engine](#engine), which orchestrates all components of the torrent engine,
- [torrent](#torrent), which coordinates the download of a torrent;
- a [piece picker](#piece-picker) per torrent, that selects the next piece to
  download;
- [peer connection](#peer-connection) in a torrent, which implement the peer
  protocol and perform the actual download (any number may exist);
- in progress [piece download](#piece-download), which track the state of an
  ongoing piece download;
- [disk IO](#disk-io), which saves downloaded file pieces.

At the end of the document, there is an [overview](#anatomy-of-a-block-fetch) of
a block fetch, incorporating the above key components.

The binary takes as command line arguments the IP address and port pairs of all
seeds to download from (must be at least one), the torrent metainfo, and sets up
a single torrent, piece picker, and peer connections to the seeds. All
IO is done asynchronously, and the torrent has a periodic update function, run
every second.


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
  multiple of 16 KiB. File pieces are indexed from zero.
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
a public method to start a torrent until completion.


## Torrent

- Contains the piece picker and connections to all seed sfrom which to download
  the torrent.
- Also contains other metadata relevant to the torrent, such as its info hash,
  the files it needs to download, the destination directory, and others.
- Torrent tick: periodically loops through all its peer connections and performs
  actions like stats collections, later choking/unchoking, resume state saving,
  and others.

### Peer sessions

A peer session is spawned on a new
[task](https://docs.rs/tokio/0.2.13/tokio/task), and torrent and the peer
session communicate with each other via async `mpsc` channels. This is
necessary as a peer connection is spawned on another task, which from the
point of view of the borrow checker is as though it were spawned on another
thread, so we can't just call methods of peer session without further
synchronization.

It would be possible to spawn the peer task
[locally](https://docs.rs/tokio/0.2.20/tokio/task/fn.spawn_local.html), but we'd
still have to use channels for communication as running the peer session and
calling its methods on the torrent task would cause lifetime issues, due to the
peer session's run method taking a mutable reference to self, thus preventing
torrent from calling any of its other methods.

As to whether running all peer sessions and torrent on the same local task
(since a peer session doesn't do much other than IO) would be more performant
over running each on a separate task is an open question and more research is
needed.

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

### Current session algorithm

A simplified version of the peer session algorithm follows.

1. Connect to the TCP socket of peer and send the BitTorrent handshake.
2. Receive and verify peer's handshake.
3. Start receiving messages.
4. Receive peer's bitfield. If the peer didn't send a bitfield, or it doesn't
   contain all pieces (i.e.  peer is not a seed), we abort the connection as
   only downloading from seed's are supported.
5. Tell peer we're interested.
6. Wait for peer to unchoke us.
7. From this point on we can start making block requests:
   1. Fill the request pipeline with the optimal number of requests for blocks
      from existing and/or new piece downloads.
   2. Wait for requested blocks.
   3. Mark requested blocks as received with their corresponding piece download.
   4. If the piece is complete, mark it as finished with the piece picker.
   5. After each complete piece, check that we have all pieces. If so, conclude
      the download, otherwise make more requests and restart.

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

So the best request queue size can be arrived at using the following formula:

```
Q = B * D / 16 KiB
```

Where B is the current download rate, D is the delay (latency) of the link, and
16 KiB is the standard block size. The download rate is continuously measured
and used to adjust this value. The latency for now is hard-coded to 1 second,
but this may be probed and dynamically adjusted too
(https://github.com/mandreyel/cratetorrent/issues/44).

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

And even from such a simple example as this, it can be concluded as a feature
and not premature optimization.

#### Slow start

Initially the request queue size is going to start from a very low value, but it
in case of fast seeders we want to max out the available bandwidth as quickly as
possible.

TCP has already solved this: it uses the [slow start
algorithm](https://blog.libtorrent.org/2011/11/requesting-pieces/) to discover
the link capacity as quickly as possible, and then enter congestion avoidance
mode. Introducing this on the BitTorrent level allows us to keep up with the
underlying TCP slow start algorithm and waste as little bandwidth as possible.

The mechanism is similar: in the beginning the session is in slow start
mode and increases the target request queue size by one with every block it
receives (doubling the queue size with each complete round trip). While in TCP
the slow start mode is left when the first timeout or packet loss occurs, at the
application layer this is less obvious and one mechanism employed by
[libtorrent](https://blog.libtorrent.org/2015/07/slow-start/) is to leave slow
start when the download rate is not increasing (significantly) anymore.
Libtorrent leaves slow start when the download rate inceases by less than 10
kB/s. Currently cratetorrent does the same.

#### Timing out requests

Each peer session needs to be able to time out requests, for two reasons:
- A peer may never send the request. Without timing out a request, we would not
  be able to attempt the download from another peer. This would cause the
  whole download to get stuck. This is the most crucial issue.
- It is also a form of optimization: if one peer doesn't send it within some
  allowed window, we can attempt to request it from another peer, which may
  result in the piece completing faster.

Each peer session has its own timeout mechanism, as each peer will have unique
download characteristics and so the timeout value needs to be dynamically
adjusted for each peer. A weighed running average is used to measure round trip
times, with new samples having moderate amount of weight (currently 1/20). This
guards against jitters. This average is used to derive the timeout value.

We don't want to time out peers instantly. For fast seeders the long tail of
request round trip times will be a few milliseconds, so any occasional hiccup
will be relatively large, which would time out the peer instantly if we didn't
use a smoothed running average. This is not desirable, as the overall round trip
time will still be low. For this reason, timeouts are minimum 2 seconds. This
number comes from tide, and further experiments are needed to prove its
viability.

### Session tick

Every second a peer session runs the update loop, which performs periodic
updates.

Each round the current download and upload rates are collected, and the session
tick collects each rounds value, calculates a running average, and resets the
per round counter.

Using this counter, it is also checked whether the session needs to escape slow
start.

Since the minimum timeout granularity is measured in seconds, the timeout
procedure is also part of the session tick.

### Seeding

Seeding is relatively uncomplicated, on the peer session level, anyway. The peer
sends us requests for a block and we check if 1) the block is correct, 2) aren't
choked and 3) aren't already requesting the same block. If these checks pass,
the peer session sends a request to the disk task to [fetch the
block](#reading-from-disk).

### Messages

The messages a peer can exchange is detailed [here](./PEER_MESSAGES.md).


## Piece download

A piece download tracks the piece completion of an ongoing piece download.

It plays an important role in optimizing download performance:
- sharing block requests in piece among peers,
- timing out peer requests,
- and end game mode to speed up the last part of the
  download.

Currently the last two are not implemented.

Piece downloads are stored in the torrent and shared with all peers in the
torrent. When a peer starts a new download, it places the download instance in
the shared torrent object. This way other peers may join this download.


## Disk IO

This entity (called `Disk` in the code, in the [`disk`
module](cratetorrent/src/disk.rs]) is responsible for everything disk storage
related. This includes:
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
  solution `Disk` is the only one accessing its own internal state.
- Worse separation of concerns, leading to code that is more difficult to reason
  about and thus potentially have more bugs. Whereas having `Disk` on a separate
  task allows for a sequential model of execution, which is clearer and less
  error-prone.

Spawning the disk task returns a command channel (aka disk handle) and a global
alert port (aka receiver): the former is used for sending commands to disk and
the latter is used to receive back the results of certain commands. However,
because `mpsc` channels can only have a single receiver and because each torrent
needs its own alert port, we use a second layer of alert channels for torrents.

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

### Torrent allocation
Since a torrent may contain multiple files, they will be downloaded in a
directory named after the torrent. There may be additional subdirectories in a
torrent, so the whole torrent's file system structure needs to be set up before
the download is begun. 

For single file downloads support the file allocation is very simple: the
to-be-downloaded file is checked for existence and the first write creates the
file.

Late we will want to pre-allocate sparse files truncated to the download length.

### Saving to disk

#### Buffering
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

#### Mapping pieces to files
When writing a piece to disk, we need to determine which file(s) the piece
belongs to. Further, a piece may span several files. This way, every time we
wish to save a piece, we need to look for its file boundaries and thus
"partition" the piece, saving the partitions to their corresponding files.

To find the files quickly, if we imagine the torrent as a single contiguous byte
stream, we can pre-compute each file's offset in the entire torrent, and compute
each piece's offset in the torrent, and use this information to check which
pieces intersect the files.

More concretely, there are two options:
- compute what pieces a file intersects with  at the beginning of a torrent, and
  store it with the file information,
- or compute what files the piece intersects with when starting a piece
  download, storing it with the in-progress piece.

The second approach is chosen for several reasons:
- the way the code is currently laid out it is easier to implement and test;
- in the first approach, to find the matching piece indices, we'd have to loop
  through all files anyway, so we don't save much computation;
- and also, there is slightly less memory overhead as we're only storing this
  data when a piece is being downloaded, not with all files in a torrent at all
  times, which is wasteful if their pieces are already downloaded.

The concrete algorithm is this:
- find the first file where
  `[file.start_offset, file.end_offset).contains(piece.start_offset)`
- if none is found, return an empty range (`[0..0)`)
- otherwise initialize the resulting file index range with
  `[found_index, found_index + 1)`
- then loop through files after the above found one, while
  `[piece.start_offset..piece.end_offset].contains(file.start_offset)` returns
  true and record the file's index in our resulting range's end
- return the resulting range 

Unless the piece is very large and there are many small files, the second part
(finding the last file intersecting with piece) should be very quick, with only
a few iterations. (This is the part that could be optimized by storing which
pieces a file intersects with in the file information.)

Then, getting the file indices is not sufficient: when writing to each file, we
also need to know where exactly in the file we need to write the part of the
piece. For each file to be written to, we query the slice of the file that
overlaps with the piece, and write to file at that position.

##### Example
Let's illustrate with an example. The first row contains 3 pieces, each 16 units
long (the precise unit doesn't matter now), and indicates their indices and
offsets within the whole torrent. The second row contains the files in the
torrent, of various length, indicating their names and their first and last byte
offsets in the torrent.

```
--------------------------------------------------------------
|0:0               |1:16                |2:32                |
--------------------------------------------------------------
|a:0,8      |b:9,19    |c:20,26  |d:27,36  |e:37,50          |
--------------------------------------------------------------
```

Then, let's assume we got piece 1 and wish to save it to disk and need to
determine which file(s) the piece belongs to.
`files_intersecting_piece(1)` should yield the files: `b`, `c`, `d`.

Another example:

```
-------------------------------------------------------
|0:0               |1:16                |2:32         |
-------------------------------------------------------
|a:0,15            |b:16,19 |c:20,23    |d:32,44      |
-------------------------------------------------------
```

Here, `files_intersecting_piece(1)` should yield the files: `b`, `c`.

#### Vectored IO
A peer downloads pieces in 16 KiB blocks and to save overhead of concatenating
these buffers these blocks are stored in the disk write buffer as is, i.e the
write buffer is a vector of byte vectors.
Writing each block to disk as a separate system call would incur tremendous
overhead (relative to the other operations in most of cratetorrent), especially
that context switches into kernel space have become more expensive lately to
mitigate risks of speculative execution, so these buffers are flushed to disk in
one system call, using [vectored
IO](http://man7.org/linux/man-pages/man2/readv.2.html).

While in most of the code asynchrony is provided for by `tokio`, all disk IO is
synchronous because `tokio_fs`'s vectored write implementation is just a wrapper
around repeatedly calling `write` for each buffer, defeating any performance
gains. For these instance of blocking IO, we make use of `tokio`'s builtin
threadpool and spawn the blocking IO
[tasks](https://docs.rs/tokio/0.2.17/tokio/task/fn.spawn_blocking.html) on it.

The [`pwritev`](https://linux.die.net/man/2/pwritev) syscall is used for the
concrete IO operation. This syscall allows one to write a vector of byte arrays
(referred to as "iovec") at a specified position in the file, without having to
make a separate [`seek`](https://linux.die.net/man/2/lseek) syscall. This means
that we do not need to write to the file in sequential order.

##### Writing blocks spanning multiple files
There is an additional complication to this: a block in a piece may span
multiple files. Therefore, we can't just pass all blocks in a piece to
`pwritev`, as that could write more than is supposed to be in the file (the
syscall writes as long as there is data in the passed in iovecs, potentially
truncating the file to a larger than desired size).

Thus, when the blocks in a piece would extend beyond a file, we need to shrink
the slice of iovecs passed to `pwritev`. To illustrate:
```
------------------
| file1 | file 2 |
--------.---------
| block .        |
--------.---------
        ^
  file boundary
```
On the first write, to `file1`, we need to trim the second part of the block.
Conversely, on the second write, to `file2`, we need to trim the first part.

There is a problem here: trimming the iovec during the first write makes us lose
the second part of the block (since we're working with slices here to
minimize allocations). There are two solutions:
- When we detect that the file boundary is in the middle of an iovec, we
  construct a new vector of iovecs, that is bounded by the length of the file
  slice that we're writing to (we can use
  [copy-on-write](https://doc.rust-lang.org/std/borrow/enum.Cow.html) to
  seamlessly handle the resulting iovecs). This avoids overwriting the original
  iovecs, at a slight cost.
- Keep metadata about the trimmed iovec, if any, and restore the trimmed part
  after the IO, [like in
  tide](https://github.com/mandreyel/tide/blob/master/src/file.cpp#L403). 

The second part is chosen for performance (as it is one of the main goals of
cratetorrent), as well as for the joy of optimizing such small and
self-contained code (this is a spare time project after all). The hairy logic is
encapsulated in the [`iovecs` module](cratetorrent/src/iovecs.rs), providing a
simple and safe abstraction.

Finally, after each write, we must trim the front of the slice of iovecs by the
number of bytes written, so that for the next file we don't write the wrong
data (otherwise we'd be writing out-of-order, i.e. wrong data to the file). This
is also taken care of by `IoVecs`.

#### Anatomy of a piece write
1. Create a new in-progress piece entry and calculate which files it spans, as
   per [the above step](#mapping-pieces-to-files).
2. Buffer all downloaded blocks in piece's write buffer.
3. When the piece is complete, remove its entry from the torrent and spawn a new
   blocking task for the hashing and write.
4. Hash the piece, and if the result is good, continue. Otherwise let torrent
   know via the alert channel of the bad piece.
5. Allocate a vector of iovecs pointing to the blocks in the piece.
6. Create a slice for this vector of iovecs; this will be adjusted with each
   write.
7. For each file that the piece spans:
  8. Get the slice of the file where we need to write the piece.
  9. Bound the iovec by the length of the file slice, optionally allocating a
     new vector of iovecs if the provided iovecs exceed the file slice's length.
  10. Write to the file at the position and for the length of the slice.
  11. Trim the slice of iovecs by the number of bytes that were written to disk.
  12. Step 10 and 11 may need to be repeated if not all bytes could be written
      to disk (as the syscall doesn't guarantee that all provided bytes can be
      written).

### Reading from disk

Reading from disk is somewhat similar to writing: we make use of a read buffer
(or cache), we use vectored IO over potentially multiple files, and use almost
the same `mpsc` infrastructure to communicate results. Therefore this is going
to be an overview of the differences.

#### Communicating results
The major difference is how results are communicated: while for block writes the
piece result is communicated to the torrent, the result of a disk read is
communicated directly to a peer. This is for two reasons:
- The torrent doesn't need to know about disk reads, so sending the read block
  first to torrent and then from torrent to the peer would not only be
  redundant, it would introduce some complexity.
- Although profiling is needed, it's likely more optimal to directly send the
  block to peer.

Now the reason I say likely is because there is a trade-off: currently peer
needs to pass a clone of its command sender with each disk read to the disk
task (otherwise the disk task wouldn't know to whom to return the block). This
incurs two updates to the atomic reference count (creation and drop), so this
solution is not entirely optimal either. For now it suffices though.

However, an _error_ is communicated to the torrent, rather than a peer session.
This is because the peer session is the wrong layer to deal with such global
errors. So the torrent is chosen. In the future it may be the engine that
handles such errors, but there is currently no real engine so this is kept in
torrent for now.

#### Read cache
The read cache is brutally simple at this step of the alpha version. For each
block request, we read in the _entire_ piece from disk, and then store it in a
hashmap in memory. That's it. Subsequent reads to blocks in the same piece will
hit the cache.

The problem here of course is that there is no eviction: so memory use could
blow up here very fast. This is one of the reasons why cratetorrent is still a
toy project (besides being very obviously in alpha).

In the future the read cache size will be configurable, as well as various other
parameters, such as how many blocks to read in when caching a piece (read cache
line size).


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


## Anatomy of seeding a block

1. `PeerSession` receives a request for a block from peer.
2. Various conditions are [checked](#seeding) and a block read request is sent
   to the disk task, along with a copy of the peer session's command sender.
3. Check if the block's piece is already in the read cache.
4. If not, the entire piece for which the block was requested is read from disk
   and placed in the read cache.
5. Return the block via the provided sender.
6. Peer eventually receives the block from the disk task and sends it to the
   peer if there hadn't been any cancel messages during this time.


## Error handling

Particular emphasis is placed on correct error handling; both internally and
the way the API consumer may handle errors.

Errors are distinguished as **fatal** and **fallible**. Fatal errors cause the
system to stop, while fallible errors are such as would occur on a time-to-time
basis (more frequently network IO failure, less frequently disk IO failure)
which should not be grounds for stopping the engine.

The distinction is made due to the ability to effortlessly propagate errors via
Rust's try (`?`) operator, as otherwise with a single `Error` type, encompassing
all possible errors of the crate (as is common in the Rust ecosystem), it would
be all too easy to forget to handle errors in critical places. For this reason
not only are the two types of errors distinct types, they don't provide
automatic conversion either.

As an example, take disk IO errors: they are generally not expected to occur,
but it could be that the user has moved the download file while the download was
ongoing, or that disk storage has been used up. This, however, should not bring
the entire engine to halt and instead these errors should be logged, routed to
the responsible entity, and the operation re-attempted at a later time point.

Now recovery is not implemented in cratetorrent yet, but the error distinction
is: there is a general `Error` type, and a `WriteError`. The disk task is
run until a fatal error is encountered (e.g. `mpsc` channel failure) and so all
it's internal methods only return `Error` on fatal error; non-fatal errors are
communicated via the alert channels. Because of this we know that in the
following code a disk write won't abort the event loop because of IO failure,
only because of channel sending failure (which is for now the desired behavior):
```rust
while let Some(cmd) = self.cmd_port.recv().await {
    match cmd {
        Command::BlockWrite(block) => {
            self.write_block(block)?;
        }
    }
}
```
