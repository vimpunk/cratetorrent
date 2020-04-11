# Research notes

This document includes notes about not yet implemented features, optimizations,
and ideas in general.

## Piece picker

In order to ensure high availability of all file pieces in a torrent network,
pieces are generally downloaded in order of _least available_ to _most
available_. Thus, for the piece picker algorithm to work correctly, it needs to
have information about the piece availability of all peers in the network.

When connecting to a peer and receiving its piece availability, the torrent's
piece availability is updated, allowing the piece picker to make a more informed
decision.

In the future, different piece download algorithms may be specified for a
torrent. E.g. some torrent engines are used for streaming video or music
content, in which case a pseudo-random download order is unhelpful, and
sequential piece picking is enabled.

## Disk IO

Once we have more torrents, each having multiple peer connections, we will need
a hashmap of torrents, and in that case each disk write (and later read) would
require synchronization of the hashmap, potentially blocking other peer
sessions.  Furthermore, in the future blocks buffered in memory for too long
might be saved to disk (before hashing) when more recently downloaded blocks are
downloaded and the above mentioned configurable write buffer upper bound is
reached. For both of these things, it would be more beneficial for `Disk` to run
its own "event loop" on a separate task, where `Disk` would receive messages
from each peer (thus not requiring synchronization of each of `Disk`'s fields),
and it could multiplex the message channel with timers.


## Choking

Since choking only becomes relevant once seeding functionality is implemented,
currently this is only saved here for the future.

Taken directly  from the protocol:

> The choking algorithm described below is the currently deployed one. It is very
> important that all new algorithms work well both in a network consisting
> entirely of themselves and in a network consisting mostly of this one.

> There are several criteria a good choking algorithm should meet. It should cap
> the number of simultaneous uploads for good TCP performance. It should avoid
> choking and unchoking quickly, known as 'fibrillation'. It should reciprocate to
> peers who let it download. Finally, it should try out unused connections once in
> a while to find out if they might be better than the currently used ones, known
> as optimistic unchoking.

> The currently deployed choking algorithm avoids fibrillation by only changing
> who's choked once every ten seconds. It does reciprocation and number of uploads
> capping by unchoking the four peers which it has the best download rates from
> and are interested. Peers which have a better upload rate but aren't interested
> get unchoked and if they become interested the worst uploader gets choked. If a
> downloader has a complete file, it uses its upload rate rather than its download
> rate to decide who to unchoke.

> For optimistic unchoking, at any one time there is a single peer which is
> unchoked regardless of its upload rate (if interested, it counts as one of the
> four allowed downloaders.) Which peer is optimistically unchoked rotates every
> 30 seconds. To give them a decent chance of getting a complete piece to upload,
> new connections are three times as likely to start as the current optimistic
> unchoke as anywhere else in the rotation.

Research paper on alternative choking algorithms:
http://bittorrent.org/bittorrentecon.pdf


## Piece download

A piece download tracks the piece completion, the peers who participate in the
  download, handles timeout logic, and mediates distributing results of a
  downloaded piece's hash test.

If a piece has a few blocks left (percentage of total piece size), waiting for
a timed out peer to send that block is wasteful and we want to request that
block from another peer in hopes it would be downloaded faster from them. If
there are more blocks left, it doesn't make sense to hand off the request,
however. See
https://github.com/mandreyel/tide/blob/master/include/tide/piece_download.hpp#L188.



## `TODO`

- continue closed peer connection
- request pipelining, including determining the best pipeline queue size based
  on current thruput rates
- optimize piece downloads by downloading a block's pieces from multiple peers
- request timeout based on single peer performance as well as general piece rtt
  when downloaded from multiple peers
- slow start mode (related to pipelining)
- optimizing disk io with read and write caches
- backpressure (source sink asynchrony)
- request timeouts
- rate limiter
- stats reporting
- parole
- uTP
- stream encryption
- only download x file(s), file padding, sparse files
- torrent queue
- Fast extension
- seeding
- NATPMP
- uPnP
- DHT
- WebTorrent
