// The assisted scripts' shared helpers (`e2e/assist.ts`) without a person or a browser: the answers typed in the terminal
// (`Person`, fed through a stream of its own, short timeouts) and the new lines of the app's prompts log (`PromptsLog`, a temp file).

import { afterEach, beforeEach, describe, expect, spyOn, test } from "bun:test";
import { appendFileSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { PassThrough } from "node:stream";
import { Person, PromptsLog, sleep } from "../e2e/assist";

let logSpy: ReturnType<typeof spyOn>;
beforeEach(() => {
  logSpy = spyOn(console, "log").mockImplementation(() => {});
});
afterEach(() => logSpy.mockRestore());

const printed = () => logSpy.mock.calls.map((c: unknown[]) => c.join(" ")).join("\n");

function person() {
  const input = new PassThrough();
  return { p: new Person(input), type: (line: string) => input.write(`${line}\n`) };
}

describe("Person: each wait owns its timer", () => {
  test("an answer typed after a finished until()'s timeout would have fired still reaches the question asked then", async () => {
    const { p, type } = person();
    // Done at once, long before its 100 ms: its line wait must not outlive it.
    expect(await p.until("waiting...", async () => true, 100)).toBe(true);
    const answer = p.yesNo("E", "Was the cursor in the field?", 2000);
    await sleep(250); // past the until()'s 100 ms
    type("y");
    expect(await answer).toBe("yes");
    expect(p.answers).toEqual([{ step: "E", question: "Was the cursor in the field?", answer: "yes" }]);
  });

  test("the same after an until() that ended because Enter was pressed, and after cancelLine()", async () => {
    const { p, type } = person();
    const skipped = p.until("waiting...", async () => false, 100);
    await sleep(20);
    type("");
    expect(await skipped).toBe(false);
    void p.line(100);
    p.cancelLine();
    const answer = p.text("E", "Anything else?", 2000);
    await sleep(250);
    type("a delay");
    expect(await answer).toBe("a delay");
  });

  test("a stale timer does not end pressed(); Enter does, and cancelLine() ends it with null", async () => {
    const { p, type } = person();
    expect(await p.until("waiting...", async () => true, 100)).toBe(true);
    const stop = p.pressed();
    let ended = false;
    void stop.then(() => (ended = true));
    await sleep(250);
    expect(ended).toBe(false);
    type("");
    expect(await stop).toBe("");

    const stop2 = p.pressed();
    p.cancelLine();
    expect(await stop2).toBeNull();
  });

  test("a question with no answer times out by its own timer; a newer line() ends the older wait", async () => {
    const { p, type } = person();
    expect(await p.yesNo("B", "Window in front?", 80)).toBe("no answer");
    const older = p.line(5000);
    const newer = p.line(5000);
    expect(await older).toBeNull();
    type("n");
    expect(await newer).toBe("n");
  });

  test("lines typed while nothing asks are dropped", async () => {
    const { p, type } = person();
    type("early");
    await sleep(20);
    const answer = p.line(2000);
    type("late");
    expect(await answer).toBe("late");
  });
});

describe("PromptsLog", () => {
  let dir = "";
  const saved = process.env.LP_PROMPTS_LOG;
  beforeEach(() => {
    dir = mkdtempSync(join(tmpdir(), "lp-assist-"));
    process.env.LP_PROMPTS_LOG = join(dir, "prompts.log");
  });
  afterEach(() => {
    rmSync(dir, { recursive: true, force: true });
    if (saved === undefined) delete process.env.LP_PROMPTS_LOG;
    else process.env.LP_PROMPTS_LOG = saved;
  });

  test("the lines read by since() are the ones printed (as the steps call it)", () => {
    const log = new PromptsLog("dev2");
    writeFileSync(log.path, "old line before the mark\n");
    log.mark();
    appendFileSync(log.path, "INFO browser prompt opened\nINFO browser prompt ended restore=Hide\n");
    const lines = log.since();
    log.print("B", lines);
    expect(lines).toEqual(["INFO browser prompt opened", "INFO browser prompt ended restore=Hide"]);
    const out = printed();
    expect(out).toContain("    INFO browser prompt opened");
    expect(out).toContain("    INFO browser prompt ended restore=Hide");
    expect(out).not.toContain("none");
    expect(out).not.toContain("old line");
  });

  test("print() alone reads since the mark; nothing new prints none", () => {
    const log = new PromptsLog("dev2");
    writeFileSync(log.path, "");
    log.mark();
    appendFileSync(log.path, "INFO raising the main window\n");
    log.print("A");
    expect(printed()).toContain("    INFO raising the main window");
    logSpy.mockClear();
    log.print("A");
    expect(printed()).toContain("none");
  });

  test("across a rotation to prompts.log.1", () => {
    const log = new PromptsLog("dev2");
    writeFileSync(log.path, "before\n");
    log.mark();
    appendFileSync(log.path, "after the mark\n");
    writeFileSync(`${log.path}.1`, "before\nafter the mark\n");
    writeFileSync(log.path, "new\n"); // shorter than the mark: rotated
    expect(log.since()).toEqual(["after the mark", "new"]);
  });
});
