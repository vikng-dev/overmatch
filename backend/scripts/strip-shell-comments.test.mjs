import { test } from "node:test";
import assert from "node:assert/strict";
import { stripShellComments } from "./strip-shell-comments.mjs";

test("launch bytes discard shell comments but preserve quoted and heredoc data", () => {
  const literal = `single='first
# literal single
last'
double="first
# literal double with \"$(printf '%s' 'nested')\"
last"
cat <<'DATA' <<-SECOND
# literal heredoc
DATA
\t# literal tab heredoc
\tSECOND
`;
  const source = `#!/bin/bash\n# documentation\n# @settings\n${literal}  # documentation\nprintf '%s' '# inline literal' # inline comment\n`;
  assert.equal(stripShellComments(source), `#!/bin/bash\n# @settings\n${literal}printf '%s' '# inline literal' # inline comment\n`);
});

test("ambiguous shell syntax refuses embedding before it can lose literal data", () => {
  for (const source of ["cat <<$DELIMITER\n# data\n", "x='unclosed\n# data\n", "cat <<DATA\n# data\n", "x=$(\n# nested evaluation\n)", "x=$'escaped\\'quote'", "x=`printf hi`\n", "echo word\\\n# literal continuation\n"]) {
    assert.throws(() => stripShellComments(source), /machine.sh:/);
  }
});
