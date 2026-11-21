// This TypeScript file is executed with Bun.
import { expect, test } from "vitest";
import { CACHE_LIMIT, TranslatedTimeline } from "./translate.ts";
import type { Task, Timeline } from "./tree.ts";

function task(title: string, note = "", id = "t1"): Task {
  return {
    done: [0, 1],
    id,
    mode: "serial",
    note,
    parent: null,
    priority: null,
    remaining: null,
    status: "pending",
    title,
    weight: 1,
  };
}

/** One plan, which is what a translation is asked for. */
function plan(tasks: readonly Task[], title = ""): Timeline {
  return { id: "tree-1", tasks, title };
}

test("a title and a note are translated, and the rest of the task is kept", async () => {
  const display = new TranslatedTimeline("ja", {
    complete: async (source: string): Promise<string> => `【${source}】`,
  });
  const tree = await display.translate(plan([task("Fix bug", "in review")]));
  expect(tree.tasks[0]).toStrictEqual({
    done: [0, 1],
    id: "t1",
    mode: "serial",
    note: "【in review】",
    parent: null,
    priority: null,
    remaining: null,
    status: "pending",
    title: "【Fix bug】",
    weight: 1,
  });
});

test("the plan's own title is translated with its tasks, and an empty one is not asked", async () => {
  const asked: string[] = [];
  const display = new TranslatedTimeline("ja", {
    complete: async (source: string): Promise<string> => {
      asked.push(source);
      return `【${source}】`;
    },
  });
  const tree = await display.translate(plan([task("Fix bug")], "Fix intake"));
  expect(tree.title).toBe("【Fix intake】");
  expect(asked).toStrictEqual(["Fix bug", "Fix intake"]);
  const untitled = await display.translate(plan([task("Fix bug")]));
  expect(untitled.title).toBe("");
});

test("an empty note is not sent for translation", async () => {
  const asked: string[] = [];
  const display = new TranslatedTimeline("ja", {
    complete: async (source: string): Promise<string> => {
      asked.push(source);
      return source;
    },
  });
  await display.translate(plan([task("Fix bug")]));
  expect(asked).toStrictEqual(["Fix bug"]);
});

test("a text already translated is not asked again", async () => {
  const asked: string[] = [];
  const display = new TranslatedTimeline("ja", {
    complete: async (source: string): Promise<string> => {
      asked.push(source);
      return `【${source}】`;
    },
  });
  await display.translate(plan([task("Fix bug")]));
  await display.translate(plan([task("Fix bug")]));
  expect(asked).toStrictEqual(["Fix bug"]);
});

test("a request that fails keeps the English, and a later paint asks again", async () => {
  let attempts = 0;
  const display = new TranslatedTimeline("ja", {
    complete: async (): Promise<string> => {
      attempts += 1;
      if (attempts === 1) {
        throw new Error("server is down");
      }
      return "【Fix bug】";
    },
  });
  const first = await display.translate(plan([task("Fix bug")]));
  expect(first.tasks[0]?.title).toBe("Fix bug");
  const second = await display.translate(plan([task("Fix bug")]));
  expect(second.tasks[0]?.title).toBe("【Fix bug】");
});

test("an answer that says nothing keeps the English", async () => {
  const answers: (string | null)[] = [null, "   "];
  const display = new TranslatedTimeline("ja", {
    complete: async (): Promise<string | null> => answers.shift() ?? null,
  });
  const tree = await display.translate(plan([task("a"), task("b")]));
  expect(tree.tasks.map((item) => item.title)).toStrictEqual(["a", "b"]);
});

test("a surrounding space in an answer is dropped", async () => {
  const display = new TranslatedTimeline("ja", {
    complete: async (): Promise<string> => "  【Fix bug】  ",
  });
  const tree = await display.translate(plan([task("Fix bug")]));
  expect(tree.tasks[0]?.title).toBe("【Fix bug】");
});

test("a server that cannot be started still lets the tree be translated", async () => {
  const display = new TranslatedTimeline("ja", {
    complete: async (source: string): Promise<string> => `【${source}】`,
    ensure: async (): Promise<boolean> => {
      throw new Error("no server");
    },
  });
  const tree = await display.translate(plan([task("Fix bug")]));
  expect(tree.tasks[0]?.title).toBe("【Fix bug】");
});

test("the cache is emptied once it is full, so a long session stays bounded", async () => {
  let asked = 0;
  const display = new TranslatedTimeline("ja", {
    complete: async (source: string): Promise<string> => {
      asked += 1;
      return `【${source}】`;
    },
    ensure: async (): Promise<boolean> => true,
  });
  const titles = Array.from(
    { length: CACHE_LIMIT + 1 },
    (_value, index) => `task ${String(index)}`,
  );
  await display.translate(plan(titles.map((title, index) => task(title, "", `t${String(index)}`))));
  expect(asked).toBe(CACHE_LIMIT + 1);
  // The first title fell out with the rest of the cache, so it is asked once more.
  await display.translate(plan([task("task 0")]));
  expect(asked).toBe(CACHE_LIMIT + 2);
});
