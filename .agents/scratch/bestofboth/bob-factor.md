# Bonded design factorization

Subject: the video game's environment destruction model: constructions holding, cracking and
collapsing. Game state only. This experiment changes no material, attachment, geometry or strength.

**Decision: keep sparse Cholesky with sparse rank-six bond downdates as the inner-solve candidate.**
The memory and solve-work kills are clear. Solve work does not grow with accumulated cracks.
Large batches should use an off-tick refactor instead of individual downdates. This is a numerical
experiment, not a claim that the complete hosted tick passes its budget. No timing runs were made.

The code is `src/construction/fracture/bondsolve/factor_experiment.rs`, compiled only in tests.
It uses the existing `Assessment::rebuild` edge law, removes every contact edge and every parallel
bearing contact, and eliminates authored buried coordinates. It retains bonded attachment slopes,
intact cohesive seams, polygon area moments and ground attachments. No replacement stiffness is
invented. `Factor::solve` gives displacements; `Edge::resultant` gives each joint's force and couple.

**MEASURED data census and storage.** Unknowns are scalar coordinates after anchor elimination.
Matrix nonzeros count the full symmetric numerical matrix; factor entries count the stored lower
triangle, including structural cancellation zeros. MB below means decimal megabytes. Storage is
the sum of actual vector capacities and object headers on this machine, not process RSS; allocator
metadata and temporary assembly/ordering workspace are excluded.

| Design | Chunks | Bonded joints | Excluded rests | Unknowns | Matrix nonzeros | Stored factor entries | Resident factor bytes / MB |
|---|---:|---:|---:|---:|---:|---:|---:|
| House | 1,122 | 3,187 | 71 | 5,772 | 115,978 | 362,202 | 7,288,224 / 7.288224 |
| Tree | 37 | 132 | 0 | 222 | 10,404 | 6,321 | 131,992 / 0.131992 |

**MEASURED:** the house has one anchored bonded component, even after excluding resting joints;
the tree also has one. The shipped bonded graph does not separate the roof into a free body.
The house has 160 buried chunks, leaving 962 moving-coordinate chunks (DERIVED from the census).
The split must follow these authored bonds rather than assume the roof is an independent owner.

**Algorithm and choice.** Exact minimum degree on chunk blocks completes each eliminated node's
neighbour clique. Each chunk block expands to its six rigid coordinates. Numeric Cholesky is
left-looking, with column storage and a temporary row incidence index. The original filled pattern
is retained after cracks: deleting bonds cannot require new fill under that fixed ordering.
MEASURED ordering adds 6,837 block edges for the house and 28 for the tree.

I chose an instrumented implementation for this experiment: it exposes every numeric operation,
preserves the filled pattern for downdates, and adds no dependency. The exact-version alternative
checked was [sprs-ldl 0.10.0](https://docs.rs/sprs-ldl/0.10.0/sprs_ldl/struct.LdlNumeric.html): its
documented `update` performs a same-pattern matrix refactor, rather than a low-rank bond removal.
This choice is justified for the experiment, not an assertion that handwritten numerical code is
preferable for production. Independent CG, analytical statics, fresh factorizations and gauge
invariance check this implementation. Bevy's pinned math re-export and glam 0.32.1's dot, cross
and matrix-vector arithmetic were verified in registry source; no new engine scheduling API is used.

**Work accounting.** All operation counts below are MEASURED loop counts or DERIVED from them.
One scalar addition, subtraction, multiplication, division or square root counts as one operation;
an FMA counts as two. This is algebraic work, not a cycle or time forecast. Integer graph work,
comparisons, allocation and data movement are not disguised as floating-point operations.
For factor column lengths `d_j`, numeric factorization costs `sum(d_j^2)`, including one square
root per column and division for each off-diagonal entry. One load column costs
`4 * (factor_entries - unknowns) + 2 * unknowns`, including both triangular solves.

| Design | Matrix assembly | Numeric factorization | Total arithmetic once per design | One solve | Two solves |
|---|---:|---:|---:|---:|---:|
| House | 4,486,566 | 33,056,342 | 37,542,908 | 1,437,264 | 2,874,528 |
| Tree | 211,230 | 203,743 | 414,973 | 24,840 | 49,680 |

DERIVED: the owner's 74 identical houses need one such immutable design factor, not 74
factorizations. Placement position/yaw changes local loads, not this local design stiffness.
The experiment builds that factor once, then reuses it across loads and topology probes; wiring
it into the asset cache is left to integration. Its library-loading harness still runs the existing
asset admission, including the old initial equilibrium; the table counts only the new kernel.
Production integration must replace that old solve, rather than keep both.

DERIVED from `Edge::resultant`, `Problem::apply` and pinned scalar math: an unprojected joint
product needs 95 operations for a ground edge, 143 for a two-ended edge. Thus the brief's current
`32 * 2 * 3,258 = 208,512` house joint visits cost **at least 19,808,640** scalar operations,
before Krylov vector updates or contact quadrature. Two direct solves plus all bonded-joint
resultants cost **at most 3,441,814** (using the larger 89-operation two-ended resultant everywhere).
The equivalent tree bounds are 802,560 versus at most 73,176. Comparing joint visits directly
with scalar operations would be invalid. Load intake/body acceleration and physics remain additional.

**Accuracy.** The observation load is gravity plus labelled fixture nonuniform lateral forces and
couples, exercising all coordinates. Reference CG uses the existing linear `Krylov`, with contacts
removed and its tighter linear convergence gate. These are aggregate Euclidean norms of the force
and couple coordinates in SI, not a per-joint relative guarantee near zero force.

| Design | Relative equation residual | Joint-field relative difference from converged CG | CG joint visits |
|---|---:|---:|---:|
| House | 2.5493e-12 | 4.4630e-11 | 7,170,750 |
| Tree | 7.6024e-14 | 2.8826e-11 | 43,692 |

DERIVED analytical fixture: a cantilever loaded sideways by 100 N at a 3 m arm has a 100 N base
shear and a 300 N m base couple. Two identical parallel bonded patches each carry half; after
one cracks, the remaining patch carries both full resultants. Both checks pass. A separate balanced
two-chunk free-body fixture gives the same 100 N internal force with either coordinate gauge and
returns two free pieces when its bond cracks. No artificial anchor reaction supplies its load.

**Cracks.** A joint is `K_e = B^T H B`, with positive translation/rotation blocks in `H`.
Factoring its small block gives `U = B^T chol(H)`, hence `K <- K - U U^T` in six rank-one steps.
Hyperbolic Cholesky downdates walk only reached columns of the retained factor. DERIVED upper bound
per crack is `36 * factor_entries + 1,165` scalar operations, including preparation of its six
vectors: **13,040,437 house**, **228,721 tree**, independently of previous crack count. The scan
also performs at most six passes over the coordinate index. Update vectors occupy six coordinate
arrays, then are discarded; no Woodbury basis or dense Schur matrix grows with crack history.

MEASURED sequence: visit bonds in authored order, skip all-fixed bonds, and accept only cracks
that preserve the component memberships and anchor states. A split candidate is returned as
`Crack::Split`, not applied to this factor. This exercises surviving bodies, not a physical prediction
of which joints should crack. House observations include 57 skipped all-fixed bonds and 104 split
candidates by the accepted 200th removal. No strength verdict chooses the sample.

| Design | Accepted cracks k | Last crack, including preparation | Cumulative updates | Two solves each later tick | Fresh numeric factorization |
|---|---:|---:|---:|---:|---:|
| House | 1 | 2,196,547 | 2,196,547 | 2,874,528 | 33,056,342 |
| House | 10 | 2,215,261 | 22,160,932 | 2,874,528 | 33,091,226 |
| House | 50 | 2,129,353 | 108,735,434 | 2,874,528 | 30,965,786 |
| House | 200 | 1,858,645 | 403,358,984 | 2,874,528 | 27,329,534 |
| Tree | 1 | 67,273 | 67,273 | 49,680 | 198,919 |
| Tree | 10 | 49,129 | 572,938 | 49,680 | 154,567 |
| Tree | 50 | 22,021 | 4,940,666 | 49,680 | 80,299 |
| Tree | 200 | Not a surviving single-body case | See below | Per resulting body | Per resulting body |

MEASURED maximum single update in these sequences: 2,225,629 house and 182,509 tree operations.
The tree has only 132 bonds. After 95 accepted removals, all 37 remaining cuts request a split;
inventing a 200-crack unsplit tree would misstate the design. Its final relative equation residual
is 3.2768e-8 and its field difference from a fresh factor is 2.7589e-8. At house k=200 those are
1.9499e-10 and 6.4554e-11. At every reported checkpoint both are below the experiment's
Resolution 1e-6 numerical accuracy gate. No result is compared with a historical crack count.

**Refactor policy.** DERIVED first-crack cost comparison, including initial assembly: a batch of
18 house cracks or seven tree cracks already favors rebuilding over that many first-cost updates.
These are workload estimates, not authored thresholds: select the cheaper operation estimate from
the current graph/reach, and run the full rebuild off the tick. By house k=200 the measured fresh
numeric cost is 27.33M, plus at most the original 4.49M assembly cost, against 403.36M individual
updates. New minimum-degree ordering can slightly increase fill (the house k=10 table shows this);
the fixed-pattern bound does not rely on new ordering improving it.

The two-solve cost stays constant through the accumulated 200 cracks. A simultaneous 200-crack
burst still costs 403.36M if processed as individual updates; that work must not all be placed on
one tick. Integration needs a versioned factor job and atomic publication of the matching topology,
with no force verdict from an obsolete factor. The job can run off the tick or advance under an
explicit work budget. Scheduling that job and retaining its matching loads are not implemented here.

**Splits and ownership.** Before numeric work, connectivity includes every live bonded edge and
authored ground/buried anchor. A changed component membership or lost anchor returns the pieces
without modifying the old factor. This is one graph traversal with removed-edge membership tests;
the experiment uses a bounded set, adding logarithmic lookup work. For a new free body, the split
balances its force/moment by rigid acceleration, fixes one coordinate gauge, and factors only its
internal unknowns. An isolated chunk has no internal solve. The free-body test checks both the
gauge and disconnection outcomes. A nonpositive numeric pivot is a bug, never a regularization or
an invented support. Each owner's solve/update is independent; dependencies within a triangular
solve/downdate require that owner's column order, not serialization of other owners.

**Kill verdict: retain this numerical candidate.** Resident factors are below the owner's 100 MB
per-design kill; two solves plus joint recovery are below even the conservative old arithmetic
floor; recurring solve cost and storage do not grow through the house's 200 cracks. The large
same-tick crack batch is a separate explicit scheduling obligation, not evidence of a cheap tick.
The experiment clones a factor for each topology probe. Production should share the immutable
design factor and pattern, with changed numeric values owned by the changed body. DERIVED even
74 full-capacity house copies would be about 539 MB, before other state; the intact case needs
only the single design factor. Allocator overhead and peak loading workspace have not been measured.

**Remaining integration:** the split adapter, immutable design cache/COW ownership, bounded
refactor publication and per-body loads/contact intake. Internal compression contacts, both closed
cracked seams and the bearing faces parallel to fixings, are excluded here. Their changing tangents
and once-per-tick policy need separate work/accuracy evidence. New bodies must be constructed at spawn from data under
ADR-0014. No physics tick pricing, collider creation, replication or visual behavior is claimed.

Reproduce with `scripts/serial-cargo test --lib --features dev_ui,bitprobe factor_experiment --
--include-ignored --nocapture`, setting `OVERMATCH_CARGO_LOCK_WAIT_S=10800`,
`OVERMATCH_UNATTENDED=1`, and `OVERMATCH_WINDOW=headless`. The script supplies the machine lock.

Verification: the three numerical tests (including the explicit observational census), strict
workspace/all-target clippy, formatting and all 48 `doc_citations`, `ui_ascii`, `net_boundary`
and `bake::` guard tests passed. The linker emits its existing large unwind-section warning on
the test binary. No runtime body/collider was added. Self-review covered all six lenses; no
backlog row names the edited files. Work remains isolated on `lane/bob-factor`; nothing was pushed.
