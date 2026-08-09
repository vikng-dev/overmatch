# Issue tracker: local markdown

Issues and PRDs live as files under `.agents/scratch/<feature-slug>/`, one directory per feature:

- the PRD is `PRD.md`
- implementation issues are `issues/<NN>-<slug>.md`, numbered from `01`
- triage state is a `Status:` line near the top of each issue file (see `triage-labels.md`)
- conversation appends to the bottom under a `## Comments` heading

"Publish to the issue tracker" means creating a file there, making the directory if needed. "Fetch the relevant ticket" means reading the path or issue number the user passed.
