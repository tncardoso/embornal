# The server

A memory works alone. One machine holds the file, and the commands read and
write it there.

A memory can also live on a server. The commands then do their work there, and
the machine that asks keeps no facts. Use this when more than one machine, or
more than one person, must write into one memory.

The server is the same code. `embornal serve` opens the memory of its machine
and puts it behind HTTP. A fact that goes through the server obeys exactly the
rules that a fact written on that machine obeys, because one piece of code
writes the facts and one piece of code reads the access rules.

## Set up the server

Make a token for each subject that will use the memory:

```bash
embornal token add alice --name laptop
```

The command shows the token one time. The memory keeps the SHA-256 of the
token, not the token, so nothing can show it again. Copy it now.

The token also gives `alice` the rules of a new user: it writes anywhere, it
reads what it wrote, and it reads the facts that the memory holds about
itself. See [The memory commands](cli.md#token).

Facts that this client writes use `alice`, the subject named by the token, as
the owner. Each fact also carries the `owner=alice` tag. A command that works
on the server file directly uses `default` and `owner=default` instead.

Then start the server:

```bash
embornal serve
```

| Flag | Effect |
| ---- | ------ |
| `--port N` | Listens on another port. The default is 1338. |
| `--bind ADDRESS` | Listens on another address. The default is `127.0.0.1`. |

The default answers this machine only. Give `--bind 0.0.0.0` to answer the
network.

## Set up the client

Write the address and the token in the `config.yaml` of the machine that asks:

```yaml
server:
  url: http://memory.example.com:1338
  token: emb_01ARZ3NDEKTSV4RRFFQ69G5FAV_...
```

To keep the token out of a file that a backup or a repository might carry, put
it in a file of its own:

```yaml
server:
  url: http://memory.example.com:1338
  token_file: ~/.config/embornal/token
```

Every `embornal memory` command then works on the server:

```bash
embornal memory store /projects/embornal "The client is a thin layer."
embornal memory recall "thin layer"
```

## A client needs no model

The server turns each fact and each question into a vector, so a client needs
none of that. Build a client without the model:

```bash
cargo install embornal --no-default-features
```

That build needs no cmake and no C++ compiler, and it downloads none of the
300 MB of weights. It is the build to install on a machine that only asks.

## What the token says

The token says which subject asks, and nothing else does. `--as-subject` works
on a memory of this machine only. A client can give the flag, but the client
ignores it and the token subject still owns new facts.

Every token subject joins the `everyone` role by default. This role reads
facts tagged with `visibility=public`. The server still requires a valid token
for every request.

To stop a token:

```bash
embornal token ls
embornal token revoke [TOKEN]
```

The next request of that client fails.

## TLS

The server speaks HTTP. Put a reverse proxy such as nginx or Caddy in front of
it for TLS, and give the client an `https://` url. A token on a plain wire is a
token that somebody can read.

## What stays on the machine that holds the memory

These commands work on the file itself, so they run there and nowhere else. A
client that tries them stops and says so.

| Command | Why |
| ------- | --- |
| `embornal serve` | It opens the file. |
| `embornal dashboard` | It reads the file. |
| `embornal memory reindex` | It writes a vector for each fact. |
| `embornal token ...` | The first token cannot come through a server. |

## When the server does not answer

The commands stop and say which address did not answer. They never fall back
to a memory of the machine that asked.

This is on purpose. A client that wrote to a file of its own would build a
second memory, and nothing brings two memories together again. A command that
fails is a smaller problem than a memory that quietly splits in two.

## How many people the server answers

The server answers one request at a time. One SQLite connection serves one
thread, and one memory speaks for one subject, so each request takes the
memory in turn.

This is enough for the agents that one person or one team runs. A memory with
many more readers needs a pool of connections.
