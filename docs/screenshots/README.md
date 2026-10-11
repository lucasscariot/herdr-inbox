# README screenshots

These are fictional example sessions. The projects, machines, task text and
agent transcript are demo data, with no access to a user's Herdr session.

Both images use the application's actual `ui::draw` renderer and default
Catppuccin theme at 120 columns by 36 rows. A temporary Rust example fed
`App::update` with session snapshots and discovery results, then captured the
styled cells with Ratatui's `TestBackend`. The inbox's example agent output
passed through the same VT100 screen parser as a live terminal.

The styled cells were exported to SVG and rasterized with `rsvg-convert`.
Each PNG is 1488 by 964 pixels, including a terminal window frame labeled
"example session". The frame is presentation around the captured UI.

- `inbox.png` shows six threads across Local and Studio, grouped by status,
  with a selected agent waiting for a decision.
- `composer.png` shows a task draft, saved presets, model and thinking choices,
  a new worktree, the comparison control and the fleet orbit.

When replacing these images, use fictional fixtures or an isolated
`herdr --session inbox-test` session. Keep the status groups and launch controls
readable, and capture the current renderer so the examples match the product.
