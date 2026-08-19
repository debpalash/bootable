# GUI and TUI parity contract

Bootable has two adapters over one product interface: GPUI and Ratatui. They must expose the same
capabilities and state transitions. Terminal geometry may reflow a card, but it must not remove a
choice, invent different behavior, or use a different information hierarchy.

## Shared order

1. Create boot media header
2. Discover bootable images, when open
3. Choose an image
4. Choose a removable drive
5. Setup options, when an image enables them
6. Review and write status/actions

The GPUI workspace stacks discovery and chooser columns below 960 px and expands catalog lists on
tall windows. Ratatui reflows at terminal breakpoints and docks Review & Write at the bottom when
the catalog is closed. Both keep branding, source, target, and the safety action visually anchored;
unused space may separate those regions but must not appear after an arbitrary document ending.

## Shared discovery states

| State | Required behavior |
| --- | --- |
| Idle | Explain the next action without pretending a request is running. |
| Loading | Keep the interface interactive and preserve already displayed cached data. |
| Ready | Show whether data was updated now or came from the cache. |
| Empty | Name what was searched and say that no matching item was found. |
| Failed | Keep cached data when available, show the failure, and expose Retry. |

Both adapters use `DiscoverySession`, `CatalogState`, `CatalogFetch`, and `CacheMode` from
`bootable-core`. The session suppresses duplicate loads and rejects a response whose distribution
slug is no longer selected.

## Cache rules

- Remote discovery is cached for 30 minutes under the operating system temporary directory.
- Manual Retry performs a network refresh instead of accepting a fresh cache entry.
- A failed refresh falls back to stale data and reports that it is stale.
- Writes use an atomic temporary file and never expose a partially serialized cache.
- Corrupt, incompatible, and unsafe cache entries are ignored; network data can repair them.

## Review rule

Both adapters consume the shared `ReviewReadiness` state and `ReviewedWriteSession`. Review remains disabled and names the
missing prerequisite until an inspected image and eligible removable target exist. A successful
Review opens the same plan hierarchy in both adapters: source, target, strategy, ordered operations,
and destructive markers. Opening Review never writes media. **Review consequences** opens a blocking
modal/overlay showing the physical target, plan-derived changes, permanent data-loss consequences,
and interruption risk. One explicit acknowledgment unlocks **Confirm erase & write**. Both adapters
then show the same phase, message, percentage, transferred bytes, throughput, ETA, elapsed time,
verification outcome, and safe-removal result. Drive refresh and app exit are locked while the write
is active.

Managed transfers follow the same rule through `ManagedDownloadSession`: one active worker, queued
retry, FIFO continuation, pause/resume persistence, cancellation cleanup, and typed terminal
outcomes are shared. GPUI async tasks and TUI threads are execution adapters only.

Any GUI or TUI change must answer these questions before merge:

- Does the other adapter expose the same control and outcome?
- Are loading, cached, empty, failure, retry, and stale-response cases equivalent?
- Is the Source → Target → Review safety order unchanged?
