// InlineSpinner.tsx - Inline diamond spinner with cycling brand colors
// Extracted from design deliverables. Cycles: cyan -> purple -> gold -> orange -> cyan.

interface InlineSpinnerProps {
	size?: number;
}

export default function InlineSpinner(props: InlineSpinnerProps) {
	const size = () => props.size ?? 16;

	return (
		<svg
			viewBox="0 0 24 24"
			width={size()}
			height={size()}
			role="img"
			aria-label="Loading"
			style="animation: ollie-spin 1.5s ease-in-out infinite;"
		>
			<title>Loading</title>
			<polygon points="12,2 22,12 12,22 2,12" fill="#4fc3f7" />
		</svg>
	);
}
