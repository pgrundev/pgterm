# pgterm Book reader — design (2026-09-06)

Read the Postgres Book (https://pgbook.dev) inside pgterm: press `b`, pick a
topic, read it, go back to your databases. pgterm surfaces the problem (a
blocked lock, a bloated table, an unused index); the book explains the
concept behind it. Same architecture as everything else in pgterm: the
terminal orchestrates a sibling CLI — here `pgbook` — and renders its JSON.

Approved approach (over fetching pgbook.dev directly): pgterm stays off the
network, and caching, offline reading and reading progress stay in one place,
so `pgbook next` in a shell continues where you stopped in pgterm.

## Two releases, in order

1. **pgbook v0.2.0** — machine-readable output: `list --json`, `read --json`.
2. **pgterm v0.2.0** — the Book mode, requiring pgbook ≥ 0.2.0.

## pgbook v0.2.0: `--json`

- `pgbook list --json` prints one JSON document and exits 0:

  ```json
  {"version": "0.1", "last_read": "locks", "topics": [ {…topic without content…} ]}
  ```

  `topics` is the index as served by pgbook.dev `/api/topics` (network, else
  the offline cache), sorted by `order`; `last_read` is the slug pgbook's own
  progress file records (`null` when nothing was read yet). Topic shape is
  `internal/topic.Topic` verbatim: `slug, title, description, level,
  reading_minutes, order, aliases, tags`.
- `pgbook read <topic> --json` prints that topic's full JSON (the same shape
  plus `content`, the lesson markdown), **records progress exactly like the
  text path** (so `next` works), never pages, exits 0. Unknown topic → the
  existing one-line `pgbook: unknown topic "x" — did you mean "y"?` on stderr,
  exit 1, empty stdout. Flag position is free (`read --json locks` works).
- Errors keep pgbook's contract: `pgbook: <message>` on stderr; exit 1 for
  failures, 2 for usage. `--json` on other commands is a usage error (2).
- No ANSI, no pager, no colour ever reaches a `--json` stream.
- Tests: `commands_test` (list/read JSON shapes, `last_read` null vs set,
  progress recorded, unknown topic), `main_test` (flag in either position,
  `--json` rejected elsewhere).
- Release: tag `v0.2.0` (existing workflow), then update the tap formula
  `Formula/pgbook.rb` (manual, from `packaging/homebrew/pgbook.rb`).
- Repo note: the pgbook checkout at `~/GPT/postgresrun/pgbook` carries
  another session's uncommitted work (tutorial rewrite, renderer step
  tracker, greeting). This change is made in a separate worktree from
  `origin/main` and only touches the `list`/`read` cases of `main.go` and
  `commands/commands.go`.

## pgterm v0.2.0: Book mode

### Discovery and version gate

- `$PGBOOK_BIN` override → `pgbook` on PATH (mirrors `PGBOT_BIN`).
- On first open of Book mode pgterm runs `pgbook --version` once and parses
  `pgbook X.Y.Z`. Missing binary → the Book screen shows the install hint
  (below), never an error dialog. Version < 0.2.0 → "pgbook 0.2.0 or newer is
  required (you have 0.1.0)" plus the same hint. Both are remembered for the
  session; `r` re-checks.
- Install hint text: `curl -fsSL https://pgbook.dev/install.sh | sh`, or
  `brew install pgrundev/tap/pgbook`, or point `PGBOOK_BIN` at it.
- `install.sh` bootstraps pgbook when missing, exactly as it does pgbot
  (`PGTERM_NO_PGBOOK=1` opts out). The Homebrew formula adds
  `depends_on "pgrundev/tap/pgbook"`.

### Runner — `src/book.rs`

The second and last place pgterm spawns a child. Closed command set:

```rust
pub enum BookCommand { Version, Index, Read(String) }   // Read(slug)
// argv: ["--version"] | ["list", "--json"] | ["read", slug, "--json"]
```

- A slug is accepted only if it matches `^[a-z0-9][a-z0-9-]{0,63}$` after
  normalising (trim, lowercase, spaces/underscores → `-`), otherwise the
  command is rejected before anything is spawned. The slug is one argv
  element, never interpolated.
- The child gets pgterm's environment **minus `DATABASE_URL` and
  `PGBOT_DATABASE_URL`**: pgbook never reads a DSN and must not receive one.
  (Profile variables like `PROD_DATABASE_URL` remain in the environment as
  they do for any child; pgbook does not read them.)
- stdin null, stdout/stderr piped, `kill_on_drop`, deadline 20 s (pgbook's
  own HTTP timeout is 10 s). Exit 0 → stdout JSON; nonzero → `SafeError`
  from the first stderr line with the `pgbook: ` prefix stripped; spawn
  failure → `ErrorKind::PgbookMissing`. Every error passes the existing
  sanitizer. New `ErrorKind` variants: `PgbookMissing` ("pgbook not found",
  Display carries the install hint like `PgbotMissing`) and `PgbookOld`.
- Book jobs run outside the monitor semaphore (they are not database checks)
  and never block the UI; at most one book job runs at a time.

### Model — in `src/book.rs`

```rust
pub struct BookIndex { pub version: String, pub last_read: Option<String>, pub topics: Vec<Topic> }
pub struct Topic { slug, title, description, level, reading_minutes: i64, order: i64,
                   aliases: Vec<String>, tags: Vec<String>, content: Option<String> }
```

`#[serde(default)]` everywhere, `null_as_empty` on every `Vec` (same lesson as
issue #3), unknown fields ignored.

### State — `BookState` on `App` (global, not per database)

```rust
pub struct BookState {
    pub status: BookStatus,            // Unchecked | Missing | Old(String) | Ready
    pub index: Option<BookIndex>,
    pub topics: HashMap<String, Topic>, // fetched this session, with content
    pub cursor: usize,                  // list selection
    pub open: Option<String>,           // slug shown in the reader
    pub scroll: HashMap<String, u16>,   // reader offset per topic
    pub pane: BookPane,                 // List | Reader (which pane has the keys)
    pub running: Option<BookCommand>,   // dedupe
    pub error: Option<SafeError>,
}
```

New `Focus::Book`. Entering it: `b` anywhere in `Focus::Main`, the `Book`
entry in the shortcut row (mouse, `Hit::OpenBook`), the command-bar verbs
`book` (open the list) and `read <topic>` (open that topic directly; an
invalid topic is an inline command-bar error before any spawn). Both verbs
are additions to the parser's closed enum. The first-run welcome screen
(no databases yet) also lists `[b] read the Postgres Book`, so pgterm is
useful before the first database is added.

Leaving it: `Esc` (from the reader, first goes back to the list; from the
list, closes Book mode) or `b` (closes immediately). `q`/Ctrl-C still quit
pgterm, as everywhere. `/` still opens the command bar; a database verb
(`inspect`, `refresh`, …) closes Book mode and runs as usual.

### Actions and effects

```rust
Action::BookFinished { cmd: BookCommand, result: Result<BookPayload, SafeError> }
// BookPayload::Version(String) | Index(BookIndex) | Topic(Topic)
Effect::SpawnBook(BookCommand)
```

`update` rules: opening Book mode with `status == Unchecked` emits
`SpawnBook(Version)`; `Ready` with no index emits `SpawnBook(Index)`; `Enter`
on a topic (or `read <slug>`) emits `SpawnBook(Read(slug))` unless it is
already in `topics`; a job in flight dedupes; results land in state, errors
show in the body with `[r] retry`; `r` in Book mode drops `index` and
`topics` and re-fetches. On index arrival the cursor lands on the topic after
`last_read` (or the first when unset), and the last-read row is marked.
Monitor ticks, tab attention glyphs and every database job continue
unchanged underneath — Book mode is a body replacement, not a modal.

### Screen — `src/screens/book.rs`

The tab row stays (attention glyphs keep working), the body is the book, the
shortcut row shows the book keys, the command bar stays.

Width ≥ 100 columns — two panes:

```
┌──────────────────────────────┬──────────────────────────────────────────────┐
│ POSTGRES BOOK  0.1           │ LOCKS                                        │
│                              │ Why a query is stuck, not slow               │
│  01 Indexes         beginner │ Intermediate · 9 min                         │
│  02 Transactions    interm.  │                                              │
│ ▸07 Locks           interm.  │ WHAT A LOCK IS                               │
│  ...                         │ Every statement takes locks …                │
│                              │     SELECT pid, wait_event_type              │
│  ◂ last read: Transactions   │     FROM pg_stat_activity …                  │
└──────────────────────────────┴──────────────────────────────────────────────┘
  j/k select · Enter open · Tab switch pane · ←/→ prev/next topic · Esc back
```

Below 100 columns the panes stack: the list, then the reader replaces it on
`Enter`, `Esc` returns to the list. The list pane is 32 columns wide.

List rows: `NN Title  level` with `▸` on the cursor, the last-read row marked
`◂`. Reader: title (bold), description, `Level · N min`, then the rendered
content, word-wrapped to the pane, scrolled with `j/k`/arrows, PgUp/PgDn,
Space, mouse wheel; the offset is remembered per topic. `←`/`→` (and `p`/`n`)
move to the previous/next topic in order and open it. Clicking a list row
opens it. A footer line shows `pgbook read <slug>` so the reader can pick the
lesson up in a shell.

### Markdown rendering — `src/screens/book.rs::render_markdown`

A port of pgbook's renderer to ratatui `Line`s, covering the subset the book
uses; anything else is plain text:

| Markdown | Rendering |
|---|---|
| `#`, `##`, `###` headings | bold, upper-cased, blank line before |
| `## Step N: Title` | bold `STEP N OF T — TITLE` (T = step headings in the doc) |
| fenced code (any language) | 4-space indent, cyan, no wrapping (long lines clip) |
| `- ` / `* ` bullets | `  • ` |
| `- [ ]` / `- [x]` | `  ☐ ` / `  ☑ ` |
| `> ` quote | 2-space indent, yellow |
| `` `code` `` inline | dim |
| `**bold**`, `*em*` | markers stripped, bold / italic |

Content comes off the network via pgbook, so before rendering every control
character except `\n` and `\t` (→ 4 spaces) is removed. Nothing in the
content can ever reach the terminal as an escape sequence.

### Keys in Book mode

```
j/k ↑/↓   move (list) / scroll (reader)     ←/→ p/n   previous / next topic
Enter     open the selected topic           Tab       switch pane (wide layout)
PgUp/PgDn Space   page the reader           r         refresh the index
Esc       back / close                      b         close
```

Number keys 1–5 are inert in Book mode (Esc first). `?` shows help, which
gains a Book section.

### Errors

| Situation | Body shows |
|---|---|
| pgbook not installed | install hint, `PGBOOK_BIN` mention, `[r] check again` |
| pgbook < 0.2.0 | version found + required, install hint |
| offline and no cache | pgbook's own message ("cannot reach pgbook.dev and no cached copy exists"), `[r] retry` |
| unknown topic via `read x` | inline command-bar error with pgbook's "did you mean" |
| child timeout / bad JSON | sanitized error, `[r] retry` |

### Demo mode

`demo/run.sh` also sets `PGBOOK_BIN` to a fake pgbook (`demo/fake-pgbook`,
shell) that serves the index and two topics from `tests/fixtures/book/`, so
the demo shows the book without network or pgbook.

### Tests

- Unit (`src/book.rs`): argv for each command; slug normalisation and
  rejection (`../x`, `locks; rm -rf /`, `--flag`, `$(id)`, 65+ chars, empty);
  version parsing (`pgbook 0.2.0` ok, `0.1.0` old, garbage → Missing-style
  error); model decode of the fixtures with `null` lists.
- Unit (`app.rs`): `b` toggles Focus, first open emits `SpawnBook(Version)`
  then `Index`; `Enter` emits `Read`; dedupe while running; cached topic
  emits nothing; `Esc` semantics (reader → list → Main); `read locks` verb
  and `book` verb; invalid slug is a cmd_error with no effect; database
  shortcuts inert; `BookFinished` error surfaces; cursor lands after
  `last_read`; monitor tick still sweeps while in Book mode.
- Unit (`parser.rs`): `book`, `read locks`, `read  Window Functions ` →
  `read window-functions`; `read` with no topic is an error; the hostile
  strings still fail.
- Unit (`render_markdown`): each row of the table above, control-character
  stripping, step counting, code lines never wrapped.
- Render (`ui.rs`, `TestBackend`): wide two-pane layout, narrow stacked
  layout, missing-pgbook body, old-version body.
- Integration (`tests/book_integration.rs`): a `fake-pgbook` bin target
  (`tests/bin/fake_pgbook.rs`, the Windows-friendly pattern from PR #1)
  answers `--version`, `list --json`, `read <slug> --json`, records argv and
  its environment; asserts the JSON round-trips, an unknown slug maps to the
  stderr message, **`DATABASE_URL` is absent from the child's environment**,
  and a missing binary yields `PgbookMissing`.

### Packaging and docs

- `install.sh`: pgbook bootstrap block next to the pgbot one.
- `packaging/homebrew/formula.sh`: `depends_on "pgrundev/tap/pgbook"`.
- README: a "Read the Postgres Book" section, the key table, `PGBOOK_BIN`,
  the demo mention; help overlay updated; `docs/design.md` gets a pointer to
  this file.
- Version 0.2.0.

## Out of scope (later)

- Findings that link to their topic ("read: locks" from a lock finding).
- Search inside pgterm (`pgbook search`), PDF download.
- Per-database reading context or annotations.
