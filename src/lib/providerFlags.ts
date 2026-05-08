export const PROVIDER_LEVEL_FLAG_LABELS: Record<string, string> = {
	"--dangerously-skip-permissions": "Bypass Permissions",
	"--dangerously-bypass-approvals-and-sandbox": "Bypass Approvals & Sandbox",
	"--dangerously-bypass-approvals": "Bypass Approvals",
	"--yolo": "YOLO Mode",
};

export function isDeniedProviderLevelFlag(arg: string): boolean {
	return arg.startsWith("--dangerously-") || arg === "--yolo";
}
