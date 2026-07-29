# Embeddings

An embedding is a list of numbers that stands for a text. Two texts with the
same sense get two lists that point the same way, even when the texts share no
word. This lets `recall` find a fact by its sense.

## The model

The memory uses EmbeddingGemma 300M, at the Q8_0 quantisation. The model runs
in this process: no server and no network call happen at the time of a search.

| Item | Value |
| ---- | ----- |
| Repository | `ggml-org/embeddinggemma-300M-GGUF` |
| File | `embeddinggemma-300M-Q8_0.gguf`, near 330 MB |
| Width | 768 numbers |
| Languages | More than 100 |

The model reads more than one language, and it puts a text and its translation
in almost the same place. A question in Portuguese therefore finds a fact in
English.

## The weights

The memory keeps the weights in `$EMBORNAL_HOME/models`. The first command
that needs them fetches them from HuggingFace and writes the progress to the
error stream:

```
$ embornal memory reindex
embornal: fetching embeddinggemma-300M-Q8_0.gguf from ggml-org/embeddinggemma-300M-GGUF
embornal: [########------------] 128 / 329 MiB
```

Each later command reads the file that is already there, so it asks the
network nothing.

To use a file of weights that is already on this machine, name it:

```yaml
embedding:
  model_path: /home/me/models/embeddinggemma-300M-Q8_0.gguf
```

## When the memory embeds a fact

`store` writes the vector of the fact at the moment that it writes the fact.

If the model fails, the fact stays. The command says so, and the fact waits in
a queue:

```
$ embornal memory store /db "The memory uses SQLite."
embornal: the fact is stored, but it has no vector: ...
```

`reindex` reads that queue. See [the commands](memory-cli.md).

## The task prefix

The model reads a prefix that names the task. The prefix is not decoration:
the model puts a question and its answer in the same place only when it knows
which of the two it reads.

| Text | What the model reads |
| ---- | -------------------- |
| A fact | `title: [PATH] | text: [CONTENT]` |
| A question | `task: search result | query: [WORDS]` |

The path of the fact is its title. The memory writes the prefix, so no caller
writes it.

## How a search mixes the two indexes

`recall` asks the keyword index and the vector index, and it mixes their
answers with the strength of each fact:

```
score = keyword_weight * keyword
      + vector_weight  * vector
      + signal_weight  * strength
```

A fact that only one index found scores zero on the other one. A fact that
both indexes name therefore rises above a fact that only one of them names.

The two scores come from different scales:

- The keyword score is a rank. bm25 says nothing on its own, because it
  depends on the words of the question and on the whole memory. The best hit
  of the answer therefore becomes 1.0, and the worst becomes 0.0. A question
  first drops the words that say nothing; see below.
- The vector score is an angle. The vectors have a length of one, so the
  distance `d` that the index gives becomes `1 - d² / 2`. This runs from 1.0,
  for a fact that says the same as the question, through 0.0 for a fact with
  nothing in common, down to -1.0 for a fact that says the opposite. The
  number holds on its own, and it needs no other hit to make sense of it.

### The words that say nothing

A word that most of the facts hold does not tell one fact from another. The
question "where is the data kept" would otherwise reach every fact that holds
"the", and the best of those weak matches would take the top of the answer
away from a fact that really answers the question.

`keyword_ceiling` sets the share of the facts that a word may hold. Above that
share, the word leaves the question. The default is 0.5.

The count runs through the index itself, and not through a list of words. Two
reasons ask for that:

- One memory holds more than one language. A list would serve only the
  language that wrote it.
- Only the index knows how it folds a word. `MEMÓRIA` and `memoria` therefore
  count as one word here, exactly as they do in the search.

A question where every word is common keeps all of its words, because a
question that holds nothing else still asks something. This also carries a
memory that is too small to tell a common word from a rare one.

The count reads the memory as it is. A memory of a few facts, written in a
terse style, may hold "is" one time only; the word then says a lot in that
memory and it stays. A memory of natural sentences holds "is" in most of them,
and it goes. The rule needs no list because the memory itself says which words
are common in it.

### The two cuts

The vector index gives the nearest facts, and it does that even when none of
them is near. Two cuts hold back what is not an answer:

- `vector_floor` drops a fact that is too far. This drops a whole answer where
  nothing is near.
- `vector_share` drops a fact that is much farther than the nearest one. This
  drops the tail of a good answer.

The model holds most of its answers between 0.05 and 0.7, so a floor near the
middle of that band would throw good answers away. The default of 0.15 keeps
the answer, and the share of 0.5 cuts the tail.

The two bands meet. A weak answer reaches 0.18, and so does a true answer to a
short question. No floor separates the two, so the default keeps the fact and
lets the order carry it.

## The configuration

```yaml
embedding:
  provider: gguf            # gguf, or none for a memory with no vectors
  model: embeddinggemma-300M-Q8_0
  repo: ggml-org/embeddinggemma-300M-GGUF
  file: embeddinggemma-300M-Q8_0.gguf
  model_path: null          # a file of weights on this machine
  dimensions: 768
  context_tokens: 2048      # how much of one fact the model reads

recall:
  keyword_weight: 1.0
  keyword_ceiling: 0.5      # a word above this share of the facts says nothing
  vector_weight: 1.0
  vector_floor: 0.15
  vector_share: 0.5
  signal_weight: 0.5
```

### A shorter vector

The model is trained so that the first numbers of a vector already carry most
of the meaning. A width below 768 therefore still works, and it makes the
index smaller. Use 512, 256 or 128.

The width of a vector index is fixed. To change `dimensions`, delete the
database or start a new one; a memory that finds another width in its file
stops instead of giving wrong answers.

### Another model

To use another GGUF model, name its repository, its file and its width. Then
write each vector again:

```
$ embornal memory reindex --all
```

## Variables

| Variable | Effect |
| -------- | ------ |
| `EMBORNAL_EMBEDDING=off` | Runs the memory on the keyword index alone. |
| `EMBORNAL_LLAMA_LOG=1` | Lets llama.cpp say what it loads and what it refuses. |

`EMBORNAL_EMBEDDING=off` needs no weights, so a machine with no network still
stores and reads facts. The tests use it.

## A build with no model

The model comes from a Cargo feature that is on by default. The feature builds
llama.cpp, which needs cmake and a C++ compiler.

```
$ cargo build --no-default-features
```

This gives a build with no model and no C++ toolchain. Such a build stops if
`config.yaml` asks for the `gguf` provider, and the message says to set
`embedding.provider` to `none`.
