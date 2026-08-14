import {
  ATTR_SERVICE_NAME,
  ATTR_SERVICE_VERSION,
} from "@opentelemetry/semantic-conventions";
import {
  DEFAULT_ENVIRONMENT,
  DEPLOYMENT_ENVIRONMENT_NAME,
  SESSION_ID,
  VCS_REF_HEAD_REVISION,
} from "./semconv";

export function webResourceAttributes(input: {
  release?: string;
  environment?: string;
  gitSha?: string;
  sessionId: string;
}): Record<string, string> {
  const attributes: Record<string, string> = {
    [ATTR_SERVICE_NAME]: "web",
    [ATTR_SERVICE_VERSION]: input.release?.trim() || "dev",
    [DEPLOYMENT_ENVIRONMENT_NAME]:
      input.environment?.trim() || DEFAULT_ENVIRONMENT,
    [SESSION_ID]: input.sessionId,
  };
  const gitSha = input.gitSha?.trim();
  if (gitSha) {
    attributes[VCS_REF_HEAD_REVISION] = gitSha;
  }
  return attributes;
}
