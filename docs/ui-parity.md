# GUI and TUI parity contract

Bootable has two adapters over one product interface: GPUI and Ratatui. They must expose the same
capabilities and state transitions. Terminal geometry may reflow a card, but it must not remove a
choice, invent different behavior, or use a different information hierarchy.

## Shared workspace

Both adapters open on the same persistent three-step path:

1. Source
2. Target
3. Review & write

Discovery and managed downloads are secondary tools. Discovery starts collapsed, loads lazily when
opened, and never displaces the Source → Target → Review hierarchy on a window or terminal large
enough to show both. Setup options appear after an image is inspected and remain collapsed until
opened. The shared step state uses exactly one active step: Source before inspection, Target after
inspection, and Review after an eligible target is explicitly selected.

The GPUI workspace stacks discovery and chooser columns below 960 px and expands catalog lists on
tall windows. Ratatui reflows at terminal breakpoints and docks Review & Write at the bottom when
the catalog is closed. Both keep branding, source, target, and the safety action visually anchored;
unused space may separate those regions but must not appear after an arbitrary document ending.

Neither adapter implicitly selects a target. Keyboard navigation, mouse input, or pointer input must
produce an explicit selection, and blocked/internal/system/read-only drives remain unselectable.
Both adapters show the same removable-media inventory summary before the device rows, followed by
the physical path, display name, capacity, eligibility, and explicit Select/Selected state.

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

Distribution search is live in both adapters and covers the name, slug, and base family. One- and
two-character terms match word prefixes so a query such as `om` finds Omarchy without treating the
middle of ChromeOS as an equally useful result. Empty results repeat the query, and failed states
rename the refresh action to Retry.

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
