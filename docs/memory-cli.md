# The memory commands

All work on the memory goes through `embornal memory`. One command sits
outside that group:

```
embornal skill
```

It writes the instructions that teach an agent to use the memory. The text
goes to standard output, so it drops into a skill file:

```
$ embornal skill > .claude/skills/memory/SKILL.md
```

The command touches no file of its own, so it answers before a memory exists.

A global flag names who asks:

```
embornal --as-subject agent memory ls
```

The default subject is `cli`. Access control reads this name.

## store

```
embornal memory store [PATH] "[CONTENT]"
```

Writes one fact to a path. The command creates each path above it that is
still absent.

| Flag | Effect |
| ---- | ------ |
| `--tag KEY=VALUE` | Adds an access tag. Repeat the flag for more tags. |

The content can hold a link in the `[[/path]]` form.

The command prints the public identifier of the fact and its path:

```
$ embornal memory store /projects/embornal "The memory lives in SQLite."
01K19XQ7M2W8Z4YB3NAV6DTCE0 /projects/embornal
```

## ls

```
embornal memory ls [PATH]
```

Lists one level below the path, the way `ls` lists a directory. With no path,
the command lists the root.

The command prints a table. Each column is as wide as its widest cell, so the
edges line up:

```
$ embornal memory ls /work
| Path            | Facts | Children |
+-----------------+-------+----------+
| /work/acme      |     4 |        1 |
| /work/notes     |    12 |        0 |
```

The path column holds the whole path, so a line reads straight into another
command. The two counts say what the path holds: facts of its own, and paths
below it. A path can hold both, because a path can be a prefix and hold
content at the same time.

A path with no child prints the heading and nothing else.

| Flag | Effect |
| ---- | ------ |
| `--plain` | Writes one path for each line, with no table. Use this in a pipe. |

The plain form marks what a path holds, as `ls -F` does:

| Mark | Meaning |
| ---- | ------- |
| `/`  | The path holds paths below it. |
| `*`  | The path holds facts of its own. |

```
$ embornal memory ls --plain /work
/work/acme/*
/work/notes*
```

## tree

```
embornal memory tree [PATH]
```

Draws the whole tree below the path. With no path, the tree starts at the
root.

```
$ embornal memory tree /projects
/projects
├── embornal*
│   └── design*
└── rust*
```

The top holds its whole path. Each path below it shows its own name only,
because the lines already say where it sits. A name that carries a `*` holds
facts of its own.

| Flag | Effect |
| ---- | ------ |
| `--dirs-only` | Shows the paths that hold paths below them, and nothing else. |

`--dirs-only` reads the tree as it stands. A path that holds one path stays,
even when that one path leaves the tree:

```
$ embornal memory tree /a --dirs-only
/a
└── branch
```

## cat

```
embornal memory cat [PATH]
```

Shows the document of one path. The facts read in the order in which they were
written.

| Flag | Effect |
| ---- | ------ |
| `--limit N` | Shows N facts only. |
| `--order-by METHOD` | `date` for the oldest first, `signal` for the strongest first. |
| `--recall` | Counts the reading as a recall, which lifts the signal. |

`cat` does not count as a recall by default. The command hands over each fact
of the path at once, so it says nothing about which fact was useful.

## recall

```
embornal memory recall [CONTENT]
```

Searches the memory. With no words, the strongest facts come back.

The command prints a table:

```
$ embornal memory recall sqlite
| Path | Signal | Fact                    |
+------+--------+-------------------------+
| /db  |  1.000 | The memory uses SQLite. |
```

The signal column holds the strength of the fact at this moment. It runs from
1.000, for a fact that somebody just read, down to 0.000 for a fact that the
memory almost lost. The column does not give the order of the answer: the
order also weighs how well each fact matches the words.

| Flag | Effect |
| ---- | ------ |
| `--limit N` | Gives N facts back. The default is in `config.yaml`. |
| `--under PATH` | Searches below this path only. |
| `--scores` | Adds the value that decided the order. |
| `--plain` | Writes one fact for each line, with no table. Use this in a pipe. |

The plain form writes the path and the fact with a tab between them, and it
writes nothing when the search found nothing.

A hit counts as a recall: it lifts the signal of the fact, and it lifts it
more when the fact was almost lost. A second search for the same fact
therefore shows a higher signal.

The search reads the keyword index and mixes its answer with the strength of
each fact. The vector index waits for an embedding provider; until then it
adds nothing.

## serve

```
embornal memory serve
```

Starts the wiki at `http://localhost:1337`. Each path is a page that holds its
facts and the paths below it. A `[[/link]]` becomes a link to that page.

| Flag | Effect |
| ---- | ------ |
| `--port N` | Listens on another port. |

Each page shows the metadata of its path below the trail: the number of facts
that the path holds, the number of paths one step below it, and the signal.
The signal is the mean strength of the facts of the path, from 1.000 for facts
that somebody read now to 0.000 for facts that the memory almost lost. A path
with no fact shows no signal.

Below its text, each fact carries its own signal, the day on which somebody
wrote it, and its tags:

```
signal 0.812 · 2026-07-28 · kind=note visibility=private
```

A path can hold a fact that somebody reads each day next to one that the
memory almost lost, so each fact states its own strength. The day is in UTC,
which is how the memory holds time. The tags are the ones that decide who
reads the fact, which include the tags that the fact takes from the paths
above it. A fact with no tag stops at the day.

On the search page, the signal is the strength that the fact had when the
search found it, before the recall lifts it.

A page does not count as a recall, but a search in the browser does.

Press Ctrl-C to stop the server.

## What a command refuses

| Case | Answer |
| ---- | ------ |
| A path with no leading `/`, a space, or `..` | The command stops and names the rule. |
| A fact on the root `/` | The root holds no facts. |
| A tag that is not `key=value` | The command stops. |
| A path that holds nothing | `ls` and `cat` say that the path is absent. |
| A subject with no policy | The commands show nothing, and a write stops. |

A fact that the policy hides does not appear in `ls`, in `cat`, in `recall` or
in the wiki. The memory does not say that the fact exists.
