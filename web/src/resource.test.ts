import { describe, expect, test } from "vitest";
import { VCS_REF_HEAD_REVISION } from "./semconv";
import { webResourceAttributes } from "./resource";

describe("web resource attributes", () => {
  test("stamps vcs.ref.head.revision when git sha is present", () => {
    const attrs = webResourceAttributes({
      release: "v1",
      environment: "playground",
      gitSha: "abc123def456",
      sessionId: "sess-1",
    });
    expect(attrs[VCS_REF_HEAD_REVISION]).toBe("abc123def456");
  });

  test("omits vcs.ref.head.revision when git sha is blank", () => {
    const attrs = webResourceAttributes({
      gitSha: "   ",
      sessionId: "sess-1",
    });
    expect(attrs[VCS_REF_HEAD_REVISION]).toBeUndefined();
  });
});
