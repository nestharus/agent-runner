import { tv } from "tailwind-variants";

export const card = tv({
	base: "max-w-[500px] rounded-lg bg-[#0f3460] p-6",
});

export const button = tv({
	base: "rounded px-5 py-2 text-sm font-medium transition-colors",
	variants: {
		intent: {
			primary: "bg-accent text-black hover:bg-accent-hover",
			secondary: "bg-[#2a3a5e] text-text hover:bg-[#0f3460]",
		},
	},
	defaultVariants: {
		intent: "primary",
	},
});

export const input = tv({
	base: "w-full rounded border bg-[#1e2a4a] px-3 py-2 text-sm text-text outline-none transition-colors focus:border-accent",
	variants: {
		error: {
			true: "border-error",
			false: "border-[#2a3a5e]",
		},
	},
	defaultVariants: {
		error: false,
	},
});

export const badge = tv({
	base: "rounded px-3 py-1.5 text-[13px]",
	variants: {
		status: {
			success: "text-success",
			error: "text-error",
			neutral: "bg-[#16213e]",
		},
	},
});
