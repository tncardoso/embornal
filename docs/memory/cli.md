# The memory commands

All work on the memory goes through `embornal memory`. Two commands sit
outside that group: `embornal bootstrap` and `embornal dashboard`.

```
embornal bootstrap
```

It writes the instructions that teach an agent to use the memory. The text
goes to standard output, and should be added to the global AGENTS.md:

```
$ embornal bootstrap >> ~/.claude/AGENTS.md
```

The command touches no file of its own, so it answers before a memory exists.

`embornal dashboard` is the other command outside the group. It starts the
wiki, and is documented in [its own section below](#dashboard).

A global flag names who asks:

```
embornal --as-subject agent memory ls
```

The default subject is `default`. Access control reads this name. Facts that
the local command line writes use `default` as their owner and carry the
`owner=default` tag.

In client mode, the command accepts `--as-subject` but ignores it. The subject
of the configured token owns each new fact.

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
| `--meta` | Shows the owner and the resolved tags below each fact. |

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
| `--meta` | Adds the owner and the resolved tags of each fact. |

The plain form writes the path and the fact with a tab between them, and it
writes nothing when the search found nothing.

With `--meta`, the table has `Owner` and `Tags` columns. The tags include the
tags that the fact takes from its paths. The `owner` tag is in this set. The
plain form writes the path, owner, tags, and fact, with a tab between each
field.

A hit counts as a recall: it lifts the signal of the fact, and it lifts it
more when the fact was almost lost. A second search for the same fact
therefore shows a higher signal.

The search reads two indexes and mixes their answers with the strength of each
fact. The keyword index finds the facts that hold the words. The vector index
finds the facts that hold the sense, so a fact answers a question that shares
no word with it:

```
$ embornal memory recall "where do my notes live"
| Path | Signal | Fact                                     |
+------+--------+------------------------------------------+
| /db  |  1.000 | The memory keeps everything in one file. |
```

A word that most of the facts hold, such as "the", tells one fact from no
other. The search drops such a word from the question, so that a fact which
holds nothing else of the question stays out of the answer.

The vector index needs a model. A memory with no model reads the keyword index
alone. See [embeddings](embeddings.md).

## reindex

```
embornal memory reindex
```

Gives a vector to each fact that has none. A fact waits for one when it was
written before the memory had a model, or when the model failed at that
moment.

```
$ embornal memory reindex
12 of 12 facts have a vector from embeddinggemma-300M-Q8_0
```

| Flag | Effect |
| ---- | ------ |
| `--limit N` | Stops after N facts. |
| `--all` | Writes the vector of every fact again. Use this after a change of model. |

The first run fetches the weights of the model, so this command is also the
way to fetch them before they are needed.

A fact that the subject may not read stays where it is.

## dashboard

```
embornal dashboard
```

Starts the wiki at `http://localhost:1337`. Each path is a page that holds its
facts and the paths below it. A `[[/link]]` becomes a link to that page.

A "Code" tab sits next to the wiki. It reads the code index of one
repository, the same way `embornal code` does. See
[The code commands](../code/cli.md#embornal-dashboard).

| Flag | Effect |
| ---- | ------ |
| `--port N` | Listens on another port. |
| `--path PATH` | Reads the code index of the repository at this path, instead of where you are. |
| `--collection NAME` | Reads this index instead of the one that `--path` names by default. |

Each page shows the metadata of its path below the trail: the number of facts
that the path holds, the total number of facts in that path and all paths
below it, the number of paths one step below it, and the signal.
The signal is the mean strength of the facts of the path, from 1.000 for facts
that somebody read now to 0.000 for facts that the memory almost lost. A path
with no fact shows no signal.

The facts of a path show newest first.

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

A sidebar sits next to the facts. It names each path one step below the
current path, with its direct fact count and its total fact count including
all paths below it. Below that, a card shows the signal of the current path
again, in large text, with the number of facts behind it. A path with no
child below it, or with no fact of its own, does not show that part of the
sidebar.

On the search page, the signal is the strength that the fact had when the
search found it, before the recall lifts it.

A page does not count as a recall, but a search in the browser does.

Press Ctrl-C to stop the wiki.

The wiki reads. It has no login, so it listens for one person on one machine.

## token

These commands run on the machine that holds the memory. The first token
cannot come through a server, because a server needs a token to answer.

```
embornal token add alice --name laptop
```

Writes a token for the subject `alice` and shows it one time. The memory keeps
the SHA-256 of the token, not the token, so nothing can show it again.

| Flag | Effect |
| ---- | ------ |
| `--name TEXT` | Says what the token is for. |
| `--expires-in DAYS` | Stops the token after that many days. |
| `--no-rules` | Writes no access rules. Use this for a subject that has its own. |

Without `--no-rules`, a new subject gets these rules:

```
p, alice,    tag:owner=alice, read,   allow
p, alice,    tag:owner=alice, write,  allow
p, alice,    tag:owner=alice, delete, allow
p, alice,    path:/*,         write,  allow
g, alice,    everyone
p, everyone, tag:visibility=public, read, allow
```

The subject thus writes anywhere, reads what it wrote, and reads public facts.
The initial facts under `/memory` are public. See
[Who owns a fact](model.md#who-owns-a-fact).

```
embornal token ls
embornal token revoke [TOKEN]
```

`ls` shows the tokens that work. `--all` shows the ones that stopped as well.
No form shows a secret, because the memory holds none.

`revoke` stops one token. The name to give is the one in the `Token` column,
which also comes inside the token itself: a token in a log says which token to
stop.

## serve

```
embornal serve
```

Puts this memory behind HTTP, so that other machines can use it. Each request
carries a token, and the token says which subject asks.

| Flag | Effect |
| ---- | ------ |
| `--port N` | Listens on another port. The default is 1338. |
| `--bind ADDRESS` | Listens on another address. The default is `127.0.0.1`. |

See [The server](server.md).

## What a command refuses

| Case | Answer |
| ---- | ------ |
| A path with no leading `/`, a space, or `..` | The command stops and names the rule. |
| A fact on the root `/` | The root holds no facts. |
| A tag that is not `key=value` | The command stops. |
| A tag with the key `owner` | The memory writes that tag itself. The command stops and the fact is not written. |
| A subject name with a space, `=` or `,` | The name becomes an access tag, so the command stops. |
| A path that holds nothing | `ls` and `cat` say that the path is absent. |
| A subject with no policy | The commands show nothing, and a write stops. |

A fact that the policy hides does not appear in `ls`, in `cat`, in `recall` or
in the wiki. The memory does not say that the fact exists.
