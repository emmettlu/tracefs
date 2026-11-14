import { handle } from "./acfs.mjs";

let text = "";
for await (const chunk of process.stdin) text += chunk;
let input = null;
try {
  input = JSON.parse(text);
} catch {
  process.stderr.write("acfs: hook input is not JSON\n");
}
if (input) {
  try {
    const out = await handle(input);
    if (out) process.stdout.write(`${JSON.stringify(out)}\n`);
  } catch (e) {
    process.stderr.write(`acfs: ${e instanceof Error ? e.message : e}\n`);
  }
}
