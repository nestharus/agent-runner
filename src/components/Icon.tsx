import type { IconDefinition } from "@fortawesome/fontawesome-common-types";

interface IconProps {
	icon: IconDefinition;
	size?: number;
	class?: string;
}

export default function Icon(props: IconProps) {
	const size = () => props.size ?? 16;
	const [width, height, , , path] = props.icon.icon;

	return (
		<svg
			xmlns="http://www.w3.org/2000/svg"
			viewBox={`0 0 ${width} ${height}`}
			width={size()}
			height={size()}
			fill="currentColor"
			class={props.class}
			aria-hidden="true"
		>
			<path d={typeof path === "string" ? path : path[0]} />
		</svg>
	);
}
