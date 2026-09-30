import { describe, expect, it } from "vitest";
import { normalizeGraphPath } from "../graph/graphProjection";
import {
  agentRef,
  entityKey,
  folderRef,
  isUnderPath,
  lexicalNormalizePath,
  parentPath,
  libraryEntryRef,
  normalizeEntityPath,
  parseEntityKey,
} from "./entityRef";

describe("entityKey / parseEntityKey", () => {
  it("round-trips ids containing separators", () => {
    // Drive letters contain ":" and library refs contain "/" — splitting on the
    // FIRST separator is what makes both survive.
    const cases = [
      folderRef("D:/Development/Wardian")!,
      libraryEntryRef("skills/dev/planner")!,
      agentRef("6f1b-uuid"),
    ];
    for (const ref of cases) {
      const parsed = parseEntityKey(entityKey(ref));
      expect(parsed).toEqual({ kind: ref.kind, id: ref.id });
    }
  });

  it("rejects malformed keys and unknown kinds", () => {
    expect(parseEntityKey("")).toBeNull();
    expect(parseEntityKey("agent")).toBeNull();
    expect(parseEntityKey("agent:")).toBeNull();
    expect(parseEntityKey(":x")).toBeNull();
    expect(parseEntityKey("memory:abc")).toBeNull();
  });

});

describe("normalizeEntityPath", () => {
  it("matches normalizeGraphPath so Garden and Graph agree on folder identity", () => {
    // Locked deliberately: the two functions are separate to avoid an import
    // cycle, so drift between them would silently split one folder into two
    // map units.
    const cases = [
      "D:\\Development\\Wardian",
      "D:/Development//Wardian/",
      "  d:\\development\\wardian  ",
      "//server/share/project",
      "/home/user/project/",
      "C:\\",
      "",
      "   ",
    ];
    for (const input of cases) {
      expect(normalizeEntityPath(input)).toBe(normalizeGraphPath(input));
    }
  });

  it("lowercases Windows and UNC paths but not POSIX paths", () => {
    expect(normalizeEntityPath("D:\\Dev\\Ward")).toBe("d:/dev/ward");
    expect(normalizeEntityPath("//Server/Share/Proj")).toBe("//server/share/proj");
    expect(normalizeEntityPath("/home/User/Proj")).toBe("/home/User/Proj");
  });
});

describe("libraryEntryRef", () => {
  it("maps each section to its kind", () => {
    expect(libraryEntryRef("skills/dev/planner")).toEqual({
      kind: "skill",
      id: "skills/dev/planner",
      source: "library",
      path: "dev/planner",
    });
    expect(libraryEntryRef("prompts/review.md")).toEqual({
      kind: "prompt",
      id: "prompts/review.md",
      source: "library",
      path: "review.md",
    });
    expect(libraryEntryRef("classes/Architect")).toEqual({
      kind: "class",
      id: "architect",
      source: "library",
      path: "architect",
    });
  });

  it("refuses automations so they cannot enter the map under a second identity", () => {
    // The whole point of the dual-identity fix: a caller holding only an
    // entry_ref must resolve to Blueprint.id rather than minting a duplicate.
    expect(libraryEntryRef("automations/build.md")).toBeNull();
  });

  it("ignores the stubbed mcps section and unknown sections", () => {
    expect(libraryEntryRef("mcps/anything")).toBeNull();
    expect(libraryEntryRef("nonsense/x")).toBeNull();
    expect(libraryEntryRef("skills")).toBeNull();
  });

  it("treats library refs case-insensitively", () => {
    expect(libraryEntryRef("Skills/Dev/Planner")?.id).toBe("skills/dev/planner");
  });
});

describe("isUnderPath and parentPath", () => {
  it("contains ordinary children and rejects a sibling sharing a prefix", () => {
    expect(isUnderPath("d:/dev/app", "d:/dev/app")).toBe(true);
    expect(isUnderPath("d:/dev/app", "d:/dev/app/src/main.rs")).toBe(true);
    expect(isUnderPath("d:/dev/app", "d:/dev/application/src/main.rs")).toBe(false);
  });

  it("contains children of a root that already ends in a separator", () => {
    // The reason this helper exists. `${root}/` builds `//` and `d://`, which
    // match none of their own children — the same bug was written independently
    // in reach containment, the attribution walk, the change-paint ancestor
    // chain, and the frontier prune.
    expect(isUnderPath("/", "/srv/app/main.rs")).toBe(true);
    expect(isUnderPath("d:/", "d:/dev/app/main.rs")).toBe(true);
    expect(isUnderPath("//server/share", "//server/share/app")).toBe(true);
  });

  it("walks to a root and stops there, keeping the root's separator", () => {
    expect(parentPath("d:/dev/app/src")).toBe("d:/dev/app");
    expect(parentPath("d:/dev")).toBe("d:/");
    expect(parentPath("d:/")).toBeNull();
    expect(parentPath("/srv/app")).toBe("/srv");
    expect(parentPath("/srv")).toBe("/");
    expect(parentPath("/")).toBeNull();
    expect(parentPath("//server/share")).toBeNull();
  });

  it("returns a parent that normalizeEntityPath still recognizes", () => {
    // `d:` and `""` would both be unusable as roots downstream, which is how the
    // truncated walks went unnoticed.
    for (const path of ["d:/dev", "/srv"]) {
      const parent = parentPath(path)!;
      expect(normalizeEntityPath(parent)).toBe(parent);
    }
  });

  it("terminates on every input", () => {
    for (const start of ["d:/a/b/c", "/a/b", "relative/path", "bare", "//server/share/x"]) {
      let cursor: string | null = start;
      let steps = 0;
      while (cursor !== null && steps < 50) {
        cursor = parentPath(cursor);
        steps += 1;
      }
      expect(cursor).toBeNull();
    }
  });
});

describe("lexicalNormalizePath", () => {
  it("resolves dot segments so containment cannot be fooled by text", () => {
    // `isUnderPath` is a prefix test, so an unresolved `..` reads as contained:
    // `d:/repo/../other/a.ts` starts with `d:/repo/` and would paint this root's
    // ancestors with churn from a sibling repository.
    expect(lexicalNormalizePath("d:/repo/../other/a.ts")).toBe("d:/other/a.ts");
    expect(isUnderPath("d:/repo", lexicalNormalizePath("d:/repo/../other/a.ts"))).toBe(false);
  });

  it("drops the segments a filesystem would have", () => {
    expect(lexicalNormalizePath("d:/repo/./src/nested/../a.ts")).toBe("d:/repo/src/a.ts");
    expect(lexicalNormalizePath("d:/repo//src///a.ts")).toBe("d:/repo/src/a.ts");
  });

  it("clamps at each root shape rather than climbing above it", () => {
    expect(lexicalNormalizePath("d:/../../x.txt")).toBe("d:/x.txt");
    expect(lexicalNormalizePath("/../../x.txt")).toBe("/x.txt");
    expect(lexicalNormalizePath("//server/share/../../x.txt")).toBe("//server/share/x.txt");
  });

  it("keeps a leading `..` on a rootless path, which still has somewhere to go", () => {
    // A relative path has nothing to clamp against, and swallowing the marker
    // would make an escaping path land inside whatever it is joined onto.
    expect(lexicalNormalizePath("../other/a.ts")).toBe("../other/a.ts");
    expect(lexicalNormalizePath("a/../../b")).toBe("../b");
  });

  it("accepts backslashes, since one arrives from git and one from the OS", () => {
    expect(lexicalNormalizePath("d:\\repo\\..\\other\\a.ts")).toBe("d:/other/a.ts");
  });
});
