import { readFileSync } from "node:fs";
import { expect, test } from "vitest";
import * as semconv from "./semconv";

type WireConstant = {
  id: string;
  typescript: string;
  value: string | null;
  values: string[] | null;
};

type WireFixture = {
  schema_version: number;
  constants: WireConstant[];
};

const fixture = JSON.parse(
  readFileSync(new URL("../../fixtures/semconv-wire-contract.json", import.meta.url), "utf8"),
) as WireFixture;

test("generated TypeScript semantic conventions match the cross-language wire fixture", () => {
  expect(fixture.schema_version).toBe(1);

  for (const constant of fixture.constants) {
    const actual = semconv[constant.typescript as keyof typeof semconv];
    expect(actual, constant.id).toEqual(constant.value ?? constant.values);
  }
});
