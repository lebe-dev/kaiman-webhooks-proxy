# Architecture

The proxy stores incoming webhooks in a SQLite queue. The HTTP receiver inserts
queued webhooks, and per-channel workers read from the same database to deliver
them. SQLite uses a connection pool and defaults to WAL.

Use a local filesystem or block-backed volume for WAL. On NFS, use rollback
journaling with `SQLITE_JOURNAL_MODE=delete` and a single replica. The filesystem
must provide reliable locking and sync operations. Use `Recreate` to stop the
old pod before its replacement opens the queue.

A local PersistentVolume binds the queue to one node. If that node becomes
unavailable, recovery requires access to the volume or a database backup.

The Helm probes use `/api/version`. A Ready pod can still fail to write to the
queue.

OAuth sessions store the username returned by the provider when available.
The UI reads it from `/api/auth/status` using the session cookie.
