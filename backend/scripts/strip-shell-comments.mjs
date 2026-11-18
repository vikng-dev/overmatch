import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

/** Strip shell comments, preserving literal lines in quotes and static heredocs.
 * Unsupported quoting/dynamic heredoc delimiters refuse embedding instead of changing data. */
export function stripShellComments(source) {
  let quote = "", continued = false;
  const contexts = [], heredocs = [], output = [];
  for (const [index, line] of source.split(/(?<=\n)/).entries()) {
    if (heredocs.length) {
      output.push(line);
      const { delimiter, tabs } = heredocs[0];
      if ((tabs ? line.replace(/^\t+/, "") : line).replace(/\n$/, "") === delimiter) heredocs.shift();
      continue;
    }
    if (!quote && !contexts.length && /^\s*#/.test(line)) {
      if (continued) throw new Error("machine.sh: comment after line continuation unsupported");
      if ((index === 0 && line.startsWith("#!")) || line === "# @settings\n") output.push(line);
      continue;
    }
    output.push(line);
    continued = false;
    for (let i = 0; i < line.length; i++) {
      const c = line[i];
      if (quote === "'") { if (c === "'") quote = ""; continue; }
      if (c === "\\" && (!quote || /[$`"\\\n]/.test(line[i + 1] ?? ""))) { continued = line[i + 1] === "\n"; i++; continue; }
      if (c === "`") throw new Error("machine.sh: backtick substitution unsupported; use $(...) on one line");
      if (c === "$" && line[i + 1] === "'") throw new Error("machine.sh: ANSI-C quoting unsupported");
      if (c === "$" && ["(", "{"].includes(line[i + 1])) {
        contexts.push({ quote, close: line[++i] === "(" ? ")" : "}", depth: 1 });
        quote = "";
        continue;
      }
      if (quote) { if (c === quote) quote = ""; continue; }
      if (c === "'" || c === '"') { quote = c; continue; }
      if (c === "#" && (i === 0 || /[\s;|&()]/.test(line[i - 1]))) break;
      const context = contexts.at(-1);
      if (context) {
        if ((c === "(" && context.close === ")") || (c === "{" && context.close === "}")) context.depth++;
        if (c === context.close && --context.depth === 0) { quote = contexts.pop().quote; continue; }
      }
      if (line.startsWith("<<<", i)) { i += 2; continue; }
      if (line.startsWith("<<", i)) {
        const match = line.slice(i).match(/^<<(-?)[ \t]*(?:'([A-Za-z_][A-Za-z0-9_]*)'|"([A-Za-z_][A-Za-z0-9_]*)"|([A-Za-z_][A-Za-z0-9_]*))(?=[\s;|&]|$)/);
        if (!match) throw new Error("machine.sh: expected static heredoc delimiter");
        heredocs.push({ delimiter: match[2] ?? match[3] ?? match[4], tabs: match[1] === "-" });
        i += match[0].length - 1;
      }
    }
    // Nested shell evaluation has its own lexer: refuse multiline substitutions rather than
    // interpreting their comment-looking data as ordinary script comments.
    if (contexts.length) throw new Error("machine.sh: multiline substitution unsupported");
  }
  if (quote || heredocs.length) throw new Error("machine.sh: unterminated quote or heredoc");
  return output.join("");
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) process.stdout.write(stripShellComments(readFileSync(0, "utf8")));
