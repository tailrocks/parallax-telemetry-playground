export type GraphqlPathPart = string | number;

type GraphqlErrorPath = Readonly<{
  path?: readonly GraphqlPathPart[];
}>;

const MAX_GRAPHQL_ERROR_COUNT = 8;
const MAX_GRAPHQL_PATH_COMPONENTS = 32;
const MAX_GRAPHQL_PATH_COMPONENT_LENGTH = 64;
const MAX_GRAPHQL_PATH_LENGTH = 128;
const MAX_GRAPHQL_ERROR_CONTEXT_LENGTH = 1024;

/** Render only bounded, telemetry-safe GraphQL path data. */
export function boundedGraphqlPath(
  path: readonly GraphqlPathPart[] | undefined,
): string {
  if (path === undefined || path.length === 0) return "<root>";

  return path
    .slice(0, MAX_GRAPHQL_PATH_COMPONENTS)
    .map((part) => {
      const value = typeof part === "number" ? String(part) : part;
      const sanitized = value
        .replace(/[^A-Za-z0-9_.:-]/g, "_")
        .slice(0, MAX_GRAPHQL_PATH_COMPONENT_LENGTH);
      return sanitized.length > 0 ? sanitized : "_";
    })
    .join(".")
    .slice(0, MAX_GRAPHQL_PATH_LENGTH);
}

/** Bound count and serialized size before GraphQL paths enter telemetry. */
export function boundedGraphqlErrorDetails(
  errors: readonly GraphqlErrorPath[],
): readonly string[] {
  const details: string[] = [];
  let serializedLength = 2;
  for (const error of errors) {
    if (details.length >= MAX_GRAPHQL_ERROR_COUNT) break;
    const path = boundedGraphqlPath(error.path);
    const separatorLength = details.length === 0 ? 0 : 1;
    const entryLength = JSON.stringify(path).length;
    if (
      serializedLength + separatorLength + entryLength >
      MAX_GRAPHQL_ERROR_CONTEXT_LENGTH
    ) {
      break;
    }
    details.push(path);
    serializedLength += separatorLength + entryLength;
  }
  return details;
}

/** Format bounded GraphQL paths for the smaller web error attribute budget. */
export function formatGraphqlErrorPaths(
  errors: readonly GraphqlErrorPath[],
  maxLength: number,
): string {
  const paths: string[] = [];
  let length = 0;
  for (const error of errors) {
    if (paths.length >= MAX_GRAPHQL_ERROR_COUNT) break;
    const path = boundedGraphqlPath(error.path);
    const separatorLength = paths.length === 0 ? 0 : 1;
    const remaining = maxLength - length - separatorLength;
    if (remaining <= 0) break;
    if (path.length > remaining && paths.length > 0) break;
    const bounded = path.slice(0, remaining);
    if (bounded.length === 0) break;
    paths.push(bounded);
    length += separatorLength + bounded.length;
    if (bounded.length < path.length) break;
  }
  return paths.join(",");
}
