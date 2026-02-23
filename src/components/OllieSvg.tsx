// OllieSvg.tsx - Ollie's portrait (geometric polyhedron head)
// Extracted from design deliverables. Animated with CSS bob + blink.

interface OllieSvgProps {
	class?: string;
	size?: number;
}

export default function OllieSvg(props: OllieSvgProps) {
	const size = () => props.size ?? 64;

	return (
		<>
			{/* Hidden boil filter definition */}
			<svg
				style="position: absolute; width: 0; height: 0; overflow: hidden;"
				aria-hidden="true"
			>
				<defs>
					<filter id="boil">
						<feTurbulence
							type="turbulence"
							baseFrequency="0.02"
							numOctaves="3"
							seed="2"
						>
							<animate
								attributeName="seed"
								values="1;2;3;4;5"
								dur="0.5s"
								repeatCount="indefinite"
							/>
						</feTurbulence>
						<feDisplacementMap in="SourceGraphic" scale="2" />
					</filter>
				</defs>
			</svg>

			{/* Portrait */}
			<svg
				viewBox="0 0 100 100"
				width={size()}
				height={size()}
				class={props.class}
				style={{
					filter: "url(#boil)",
					animation: "ollie-bob 3s ease-in-out infinite",
				}}
			>
				<g class="portrait-head" transform="translate(0, 5)">
					<polygon points="15,20 85,30 75,60 5,50" fill="#ff8a65" />
					<polygon points="5,50 75,60 45,95 -5,80" fill="#4fc3f7" />
					<polygon points="75,60 85,30 95,80 45,95" fill="#b388ff" />
					<line
						x1="5"
						y1="50"
						x2="75"
						y2="60"
						stroke="#121212"
						stroke-width="2"
						opacity="0.2"
					/>
					<line
						x1="85"
						y1="30"
						x2="75"
						y2="60"
						stroke="#121212"
						stroke-width="2"
						opacity="0.2"
					/>
					<line
						x1="45"
						y1="95"
						x2="75"
						y2="60"
						stroke="#121212"
						stroke-width="2"
						opacity="0.2"
					/>
					<g
						class="portrait-eyes"
						style="animation: ollie-blink 4s ease-in-out infinite;"
					>
						<circle cx="55" cy="75" r="5" fill="#121212" />
						<polygon points="75,68 80,70 78,75 73,73" fill="#121212" />
					</g>
				</g>
			</svg>
		</>
	);
}
