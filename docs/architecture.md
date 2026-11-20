# Architecture

The proxy stores incoming webhooks in a SQLite queue. The HTTP receiver inserts
queued webhooks, and per-channel workers read from the same database to deliver
them. SQLite uses WAL with a connection pool.

Deploy SQLite on a local filesystem or a block-backed persistent volume. SQLite
WAL does not support access from multiple hosts over NFS. A rolling update can
run two pods against the same volume even with one replica. Use `Recreate` to
stop the old pod before starting its replacement.

A local PersistentVolume binds the queue to one node. If that node becomes
unavailable, recovery requires access to the volume or a database backup.

The Helm probes use `/api/version`. A Ready pod can still fail to write to the
queue.

OAuth sessions store the username returned by the provider when available.
The UI reads it from `/api/auth/status` using the session cookie.
