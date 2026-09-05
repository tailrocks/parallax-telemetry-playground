import { describe, expect, test } from "vitest";
import {
  boundedGraphqlErrorDetails,
  boundedGraphqlPath,
  formatGraphqlErrorPaths,
} from "./graphql-errors";

describe("GraphQL telemetry path bounds", () => {
  test("sanitizes and bounds one path", () => {
    const sanitized = boundedGraphqlPath(["query", "field\nwith-secret"]);
    const bounded = boundedGraphqlPath([
      "query",
      "x".repeat(200),
      "field",
    ]);

    expect(sanitized).toBe("query.field_with-secret");
    expect(bounded.length).toBeLessThanOrEqual(128);
  });

  test("bounds error count and serialized context", () => {
    const errors = Array.from({ length: 20 }, (_, index) => ({
      path: [`field-${index}`, "nested"],
    }));

    const details = boundedGraphqlErrorDetails(errors);

    expect(details).toHaveLength(8);
    expect(JSON.stringify(details).length).toBeLessThanOrEqual(1024);
  });

  test("formats the smaller error attribute budget", () => {
    const errors = [
      { path: ["first"] },
      { path: ["second"] },
      { path: ["third"] },
    ];

    const formatted = formatGraphqlErrorPaths(errors, 15);

    expect(formatted.length).toBeLessThanOrEqual(15);
    expect(formatted).toBe("first,second");
  });
});
