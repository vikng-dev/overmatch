# Lens: brevity

Subject: how the code and its comments say what they say.

Principle: `AGENTS.md`, "Say each thing once, in the fewest words that keep it true". Every line is
read by every later agent and reviewer: a line that tells the reader nothing new spends their
attention and drifts from the code. Line count is evidence, never a target.

## Finding classes

Comments first: they are where this tree carries most of its excess.

- **Restated code**: the comment says what the code beside it says: the item's name as a sentence,
  its type, the calls in order, the arithmetic. Delete it.
- **Padding**: a true comment in more words than its content: a recap of the paragraph above, a
  hedge, "note that", a summary of the summary, a field doc restating its type's doc. Cut it to the
  sentence that carries the content.
- **Repeated why**: one reason written at several sites. It lives on the item that owns it; the
  other sites drop it or name that item.
- **Edit history** and **retold narrative** (`AGENTS.md`'s comment rules): keep the current truth,
  drop the label; a story a design doc owns becomes a pointer to it.

Code:

- **Knowledge stated twice**: two sites encode one fact and must change together. Give the fact one
  owner. Similar shape alone is not this.
- **Shallow abstraction**: a wrapper, type or module that forwards and hides nothing. Inline it,
  unless an invariant, an ownership line or a real seam goes with it.
- **Needless intermediate**: a binding, conversion, clone or allocation that adds no meaning, reuse
  or required lifetime. Use the value directly.
- **Hand-written standard operation**: a loop or match that spells out an iterator method, an
  `Option`/`Result` combinator or an expression-valued `if`/`match`. Use the standard form where it
  reads at least as clearly.
- **Test ceremony**: repeated setup that hides the varying input and the asserted invariant. A local
  fixture constructor or a case table; every distinct assertion stays.
- **Dead code**: an item no caller, registration, script, feature or platform reaches. Delete it and
  what only it used.

## Not a finding

- A comment stating an invariant, a unit, an ordering constraint, a reason, a rejected
  alternative, a counterexample, a measured bound or a MEASURED/DERIVED label. Its wording may be a
  padding finding; its content stays.
- A version-pinned upstream citation (crate, version, file and lines): the next engine bump rechecks
  the claim by it.
- The reason behind player-visible copy: the next copy trim has nothing else to keep a clause by.
- A protective newtype, an intermediate whose name carries a unit or a derivation, a reused scratch
  buffer.
- Similar code stating different knowledge: sim and view, server and client.
- Explicit failure handling. A shorter form through `.ok()`, `unwrap_or` or a dropped arm is an
  `error-handling` finding, not a saving.
- A tripwire test whose apparent tautology catches an upstream change.
- Fewer lines from denser formatting or a macro that hides them.

## Fix

Delete, or rewrite to the fewest words that keep the content. Each finding names what the rewrite
keeps and its net lines saved (DERIVED, after any helper it adds). A comment stating a rule the code
does not hold is not brevity's to delete: report it under the lens the rule belongs to.
