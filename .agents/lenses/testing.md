# Lens: testing

Subject: what the tests prove about the game's behaviour and how it is put together. Where they
touch the damage model, game state only.

Principle: `AGENTS.md`, "A test is written under scrutiny" and "Pin sourced facts". A test names the
invariant a plausible mutant would break and who relies on it, and tolerates every permitted change
elsewhere.

## Finding classes

- **Coupled assertion**: a test pins a value another module owns (a taste, a unit, a timing, a
  representation) beyond the invariant it names, so a permitted change there turns it red with no
  contract broken. Assert the owned relationship or outcome; read an authored parameter from its
  owner and derive the expectation independently, never by calling the code under test for its
  answer.
- **Substituted composition**: a test claims shipped registration, ordering, gating or integration
  but rebuilds that wiring in its harness, so deleting the shipped guarantee leaves the test green,
  or the harness lacks a reader the shipped app has. Mount the production plugin or installer that
  owns the guarantee; control time and outside input around it.
- **Historical oracle**: a test holds an emergent output, the result of several laws together, to a
  number, window or stored result whose only source is an earlier run or a guess, so an honest law
  change turns it red or gets it overwritten. Signatures: a bound beside a `MEASURED` value with no
  derivation; a stored file every law change blesses. Derive the expectation from the law and its
  inputs, with premises and numerical allowance independent of the observed answer; or assert a
  relation (below and above a limit, symmetry, conservation, monotonicity); or state it as an owner's
  decision with its conditions; otherwise print it as an observation. Widening a window read off a
  run does not make it honest.

## Not a finding

- A minimal app or a direct system call proving a deliberately narrow claim.
- An exact value the tested law or a declared contract justifies on its own.
- Labelled fixture data asserting a class invariant.
- A tripwire naming the upstream contract or workaround it guards.
- A deliberately different composition that probes an upstream defect.
- Similar tests with distinct failure conditions.
- A player requirement or frame budget the owner decided, with its conditions stated.
- A bug's witness that states the invariant the defect broke.
- A wire or file format pin: an interface, not an output.

Asset-specific pass conditions are `base-truths`'; setup that hides the varying input is
`brevity`'s test ceremony; redundant tests follow `.agents/docs/code-quality-standard.md` A5.

## Fix

Name the production defect the test must catch and the permitted change it must survive; repair the
assertion or the harness to match. Delete a test only after showing another supplies its evidence.
