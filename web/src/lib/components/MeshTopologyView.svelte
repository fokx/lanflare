<script lang="ts">
	import { onMount } from 'svelte';
	import { appState } from '$lib/state.svelte';
	import { shortenFingerprint, formatBytes } from '$lib/format';
	import type { DiscoveredPeer, MeshNode, MeshLink } from '$lib/types';
	import {
		Network,
		Route,
		Gauge,
		Layers,
		Laptop,
		Smartphone,
		Server,
		Globe,
		Radio,
		RefreshCw,
		Play,
		Scissors,
		Link2,
		Copy,
		Check,
		AlertTriangle,
		CheckCircle2,
		XCircle,
		Send,
		SlidersHorizontal,
		RotateCcw,
		Zap
	} from 'lucide-svelte';

	let { onSelectPeer } = $props<{
		onSelectPeer?: (peer: DiscoveredPeer) => void;
	}>();

	// Simulation Configuration State
	let simNodeCount = $state<number>(3);
	let simSubnetMode = $state<'dual' | 'triple' | 'single'>('dual');
	let simLatencyMs = $state<number>(15);
	let isSpawning = $state<boolean>(false);
	let copiedFp = $state<string | null>(null);

	// Node Positions for SVG Canvas Layout (key: fingerprint)
	let nodePositions = $state<Record<string, { x: number; y: number }>>({});
	let draggingNodeFp = $state<string | null>(null);
	let dragOffset = $state<{ x: number; y: number }>({ x: 0, y: 0 });

	// Viewbox dimensions
	const svgWidth = 840;
	const svgHeight = 480;

	onMount(async () => {
		await appState.loadMeshTopology();
		initLayout();
	});

	// Auto-layout nodes cleanly when topology changes or initializes
	function initLayout() {
		const topology = appState.meshTopology;
		if (!topology || topology.nodes.length === 0) return;

		const localFp = topology.localFingerprint?.toUpperCase();
		const nodes = topology.nodes;
		const nextPositions: Record<string, { x: number; y: number }> = {};

		// Place local node at center
		const centerX = svgWidth / 2;
		const centerY = svgHeight / 2;
		if (localFp) {
			nextPositions[localFp] = { x: centerX, y: centerY };
		}

		// Other nodes arranged radially around center
		const otherNodes = nodes.filter((n) => n.fingerprint.toUpperCase() !== localFp);
		const count = otherNodes.length;

		if (count === 1) {
			nextPositions[otherNodes[0].fingerprint.toUpperCase()] = {
				x: centerX + 200,
				y: centerY
			};
		} else if (count === 2) {
			nextPositions[otherNodes[0].fingerprint.toUpperCase()] = {
				x: centerX - 220,
				y: centerY
			};
			nextPositions[otherNodes[1].fingerprint.toUpperCase()] = {
				x: centerX + 220,
				y: centerY
			};
		} else {
			const radius = Math.min(220, Math.max(140, 70 + count * 25));
			otherNodes.forEach((node, idx) => {
				const angle = (idx / count) * 2 * Math.PI - Math.PI / 2;
				nextPositions[node.fingerprint.toUpperCase()] = {
					x: Math.round(centerX + radius * Math.cos(angle)),
					y: Math.round(centerY + radius * Math.sin(angle))
				};
			});
		}

		nodePositions = nextPositions;
	}

	// Watch for topology nodes changes to update layout if new nodes appear
	$effect(() => {
		const topology = appState.meshTopology;
		if (topology && topology.nodes.length > 0) {
			let missing = false;
			for (const n of topology.nodes) {
				if (!nodePositions[n.fingerprint.toUpperCase()]) {
					missing = true;
					break;
				}
			}
			if (missing) {
				initLayout();
			}
		}
	});

	// Get position with safe fallback
	function getNodePos(fp: string): { x: number; y: number } {
		const upper = fp.toUpperCase();
		return nodePositions[upper] || { x: svgWidth / 2, y: svgHeight / 2 };
	}

	// Node Dragging handlers
	function handleNodePointerDown(e: PointerEvent, fp: string) {
		draggingNodeFp = fp.toUpperCase();
		const pos = getNodePos(fp);
		const svgRect = (e.currentTarget as SVGGraphicsElement).ownerSVGElement?.getBoundingClientRect();
		if (svgRect) {
			const scaleX = svgWidth / svgRect.width;
			const scaleY = svgHeight / svgRect.height;
			const clientXInSvg = (e.clientX - svgRect.left) * scaleX;
			const clientY = (e.clientY - svgRect.top) * scaleY;
			dragOffset = { x: clientXInSvg - pos.x, y: clientY - pos.y };
		}
		(e.target as Element).setPointerCapture(e.pointerId);
	}

	function handleSvgPointerMove(e: PointerEvent) {
		if (!draggingNodeFp) return;
		const svg = e.currentTarget as SVGSVGElement;
		const rect = svg.getBoundingClientRect();
		const scaleX = svgWidth / rect.width;
		const scaleY = svgHeight / rect.height;
		const clientXInSvg = (e.clientX - rect.left) * scaleX;
		const clientY = (e.clientY - rect.top) * scaleY;

		const nextX = Math.max(50, Math.min(svgWidth - 50, clientXInSvg - dragOffset.x));
		const nextY = Math.max(50, Math.min(svgHeight - 50, clientY - dragOffset.y));

		nodePositions[draggingNodeFp] = { x: nextX, y: nextY };
	}

	function handleSvgPointerUp() {
		draggingNodeFp = null;
	}

	// Active selected node
	let selectedNode = $derived.by<MeshNode | null>(() => {
		const topology = appState.meshTopology;
		if (!topology || topology.nodes.length === 0) return null;
		const targetFp = appState.selectedMeshNodeFp?.toUpperCase();
		if (!targetFp) return topology.nodes[0];
		return (
			topology.nodes.find((n) => n.fingerprint.toUpperCase() === targetFp) ||
			topology.nodes[0]
		);
	});

	// Deduplicated undirected links for rendering
	let renderedLinks = $derived.by(() => {
		const topology = appState.meshTopology;
		if (!topology) return [];

		const pairMap = new Map<
			string,
			{
				key: string;
				srcFp: string;
				dstFp: string;
				rttMs: number;
				isSevered: boolean;
				subnetTag?: string;
			}
		>();

		for (const link of topology.links) {
			const u = link.srcFp.toUpperCase();
			const v = link.dstFp.toUpperCase();
			const key = u < v ? `${u}:${v}` : `${v}:${u}`;

			const isSevered =
				appState.severedLinks.includes(`${u}:${v}`) ||
				appState.severedLinks.includes(`${v}:${u}`);

			if (!pairMap.has(key)) {
				pairMap.set(key, {
					key,
					srcFp: link.srcFp,
					dstFp: link.dstFp,
					rttMs: link.rttMs,
					isSevered,
					subnetTag: link.subnetTag
				});
			}
		}

		return Array.from(pairMap.values());
	});

	// Dijkstra Shortest Path calculation from Local Node to Selected Node
	let dijkstraRoute = $derived.by(() => {
		const topology = appState.meshTopology;
		if (!topology || !selectedNode) return null;

		const localFp = topology.localFingerprint.toUpperCase();
		const targetFp = selectedNode.fingerprint.toUpperCase();

		if (localFp === targetFp) {
			return {
				hops: 0,
				totalLatencyMs: 0,
				pathFps: [localFp],
				pathAliases: [selectedNode.alias],
				reachable: true
			};
		}

		// Adjacency list: node -> array of { neighbor, rtt }
		const adj = new Map<string, Array<{ neighbor: string; rtt: number }>>();
		for (const node of topology.nodes) {
			adj.set(node.fingerprint.toUpperCase(), []);
		}

		for (const link of renderedLinks) {
			if (link.isSevered) continue; // Skip severed links
			const u = link.srcFp.toUpperCase();
			const v = link.dstFp.toUpperCase();
			adj.get(u)?.push({ neighbor: v, rtt: link.rttMs });
			adj.get(v)?.push({ neighbor: u, rtt: link.rttMs });
		}

		const relayMap = new Map<string, boolean>();
		const aliasMap = new Map<string, string>();
		for (const node of topology.nodes) {
			const fp = node.fingerprint.toUpperCase();
			relayMap.set(fp, node.relayEnabled);
			aliasMap.set(fp, node.alias);
		}

		// Dijkstra
		const dist = new Map<string, number>();
		const prev = new Map<string, { node: string; rtt: number }>();
		const visited = new Set<string>();

		dist.set(localFp, 0);

		while (true) {
			let u: string | null = null;
			let minD = Infinity;

			for (const [node, d] of dist.entries()) {
				if (!visited.has(node) && d < minD) {
					minD = d;
					u = node;
				}
			}

			if (!u || minD === Infinity) break;
			if (u === targetFp) break;

			visited.add(u);

			// Intermediate relay check (ADR-029)
			if (u !== localFp && !relayMap.get(u)) {
				continue; // Non-relay intermediate nodes cannot route traffic
			}

			const neighbors = adj.get(u) || [];
			for (const edge of neighbors) {
				if (visited.has(edge.neighbor)) continue;
				const alt = minD + 15 + edge.rtt; // 15ms hop penalty (DEFAULT_HOP_PENALTY_MS)
				if (alt < (dist.get(edge.neighbor) ?? Infinity)) {
					dist.set(edge.neighbor, alt);
					prev.set(edge.neighbor, { node: u, rtt: edge.rtt });
				}
			}
		}

		if (!dist.has(targetFp) || dist.get(targetFp) === Infinity) {
			return {
				hops: 0,
				totalLatencyMs: 0,
				pathFps: [],
				pathAliases: [],
				reachable: false
			};
		}

		// Reconstruct path
		const pathFps: string[] = [];
		let curr: string | undefined = targetFp;
		let cumulativeLatency = 0;

		while (curr) {
			pathFps.unshift(curr);
			const p = prev.get(curr);
			if (p) {
				cumulativeLatency += p.rtt;
				curr = p.node;
			} else {
				break;
			}
		}

		const pathAliases = pathFps.map((fp) => aliasMap.get(fp) || shortenFingerprint(fp));

		return {
			hops: pathFps.length - 1,
			totalLatencyMs: cumulativeLatency,
			pathFps,
			pathAliases,
			reachable: true
		};
	});

	// Trigger simulation launch
	async function handleSpawnSimulation() {
		isSpawning = true;
		try {
			const subnets =
				simSubnetMode === 'triple'
					? ['SubnetA', 'SubnetB', 'SubnetC']
					: simSubnetMode === 'dual'
						? ['SubnetA', 'SubnetB']
						: ['SubnetA'];

			await appState.startMeshSimulation({
				nodeCount: simNodeCount,
				subnets,
				latencyMs: simLatencyMs
			});
			initLayout();
		} finally {
			isSpawning = false;
		}
	}

	// Trigger Diagnostics
	async function handleRunTraceroute() {
		if (!selectedNode) return;
		await appState.runMeshTraceroute(selectedNode.fingerprint);
	}

	async function handleRunBenchmark() {
		if (!selectedNode) return;
		await appState.runMeshBenchmark(selectedNode.fingerprint);
	}

	// Quick link severing
	async function handleToggleLink(fromFp: string, toFp: string) {
		await appState.toggleMeshLink(fromFp, toFp);
	}

	function handleCopyFp(fp: string) {
		navigator.clipboard.writeText(fp);
		copiedFp = fp;
		setTimeout(() => {
			if (copiedFp === fp) copiedFp = null;
		}, 2000);
	}

	function handleSendFiles(node: MeshNode) {
		if (onSelectPeer) {
			const peer: DiscoveredPeer = {
				fingerprint: node.fingerprint,
				alias: node.alias,
				version: '2.2',
				deviceModel: 'Mesh Node',
				deviceType: 'Desktop',
				endpoints: [],
				protocol: 'https',
				port: 53317,
				download: true,
				sources: 1,
				lastSeen: node.lastSeenMs
			};
			onSelectPeer(peer);
		}
	}
</script>

<div class="flex-1 flex flex-col h-full overflow-hidden bg-slate-50 dark:bg-slate-950">
	<!-- Top Bar / Simulator Control Sandbox -->
	<header
		class="flex-none p-3.5 bg-white dark:bg-slate-900/90 border-b border-slate-200 dark:border-slate-800 shadow-xs z-10 space-y-3"
	>
		<div class="flex flex-wrap items-center justify-between gap-3">
			<div class="flex items-center gap-2.5">
				<div class="p-2 rounded-xl bg-indigo-500/10 text-indigo-600 dark:text-indigo-400">
					<Network class="w-5 h-5 stroke-[2.2]" />
				</div>
				<div>
					<div class="flex items-center gap-2">
						<h2 class="text-sm font-bold text-slate-900 dark:text-white">
							Interactive Mesh Topology Inspector
						</h2>
						<span
							class="text-[10px] font-mono px-1.5 py-0.5 rounded-full bg-emerald-100 dark:bg-emerald-500/20 text-emerald-700 dark:text-emerald-400 font-semibold"
						>
							BGP-Style Path Vector
						</span>
					</div>
					<p class="text-[11px] text-slate-500 dark:text-slate-400">
						Real-time visualization of 1-hop neighbours, multi-subnet bridge relays, and multi-hop overlay paths
					</p>
				</div>
			</div>

			<!-- Simulator Controls -->
			<div class="flex flex-wrap items-center gap-2">
				<div class="flex items-center gap-1.5 bg-slate-100 dark:bg-slate-800/80 rounded-lg p-1 border border-slate-200 dark:border-slate-700/60 text-xs">
					<span class="text-[11px] text-slate-500 font-medium pl-1.5">Nodes:</span>
					{#each [2, 3, 4, 5] as count (count)}
						<button
							onclick={() => (simNodeCount = count)}
							class="px-2 py-0.5 rounded text-xs font-mono font-medium transition-all {simNodeCount === count
								? 'bg-white dark:bg-slate-700 text-indigo-600 dark:text-indigo-300 shadow-xs'
								: 'text-slate-600 dark:text-slate-400 hover:text-slate-900 dark:hover:text-white'}"
						>
							{count}
						</button>
					{/each}
				</div>

				<div class="flex items-center gap-1.5 bg-slate-100 dark:bg-slate-800/80 rounded-lg p-1 border border-slate-200 dark:border-slate-700/60 text-xs">
					<span class="text-[11px] text-slate-500 font-medium pl-1.5">Subnets:</span>
					<button
						onclick={() => (simSubnetMode = 'dual')}
						class="px-2 py-0.5 rounded text-xs font-medium transition-all {simSubnetMode === 'dual'
							? 'bg-white dark:bg-slate-700 text-indigo-600 dark:text-indigo-300 shadow-xs'
							: 'text-slate-600 dark:text-slate-400'}"
						title="Cross-Subnet 2-Hop Bridge Relay"
					>
						Dual
					</button>
					<button
						onclick={() => (simSubnetMode = 'triple')}
						class="px-2 py-0.5 rounded text-xs font-medium transition-all {simSubnetMode === 'triple'
							? 'bg-white dark:bg-slate-700 text-indigo-600 dark:text-indigo-300 shadow-xs'
							: 'text-slate-600 dark:text-slate-400'}"
						title="3-Hop Chain across 3 Subnets"
					>
						Triple
					</button>
					<button
						onclick={() => (simSubnetMode = 'single')}
						class="px-2 py-0.5 rounded text-xs font-medium transition-all {simSubnetMode === 'single'
							? 'bg-white dark:bg-slate-700 text-indigo-600 dark:text-indigo-300 shadow-xs'
							: 'text-slate-600 dark:text-slate-400'}"
						title="Single Layer 2 LAN"
					>
						Single
					</button>
				</div>

				<button
					onclick={handleSpawnSimulation}
					disabled={isSpawning}
					class="flex items-center gap-1.5 px-3 py-1.5 rounded-lg bg-indigo-600 hover:bg-indigo-500 text-white font-medium text-xs shadow-xs hover:shadow-indigo-500/20 transition-all cursor-pointer disabled:opacity-50"
				>
					<Play class="w-3.5 h-3.5 fill-white/20 {isSpawning ? 'animate-spin' : ''}" />
					<span>{isSpawning ? 'Spawning Cluster...' : 'Spawn Mesh Sandbox'}</span>
				</button>

				<button
					onclick={initLayout}
					class="p-1.5 rounded-lg text-slate-600 dark:text-slate-400 hover:text-slate-900 dark:hover:text-white hover:bg-slate-100 dark:hover:bg-slate-800 transition-colors"
					title="Reset Node Layout"
				>
					<RotateCcw class="w-4 h-4" />
				</button>
			</div>
		</div>

		<!-- Quick Link Sever / Restore Matrix Pills -->
		{#if renderedLinks.length > 0}
			<div class="flex flex-wrap items-center gap-1.5 pt-1 border-t border-slate-100 dark:border-slate-800/80">
				<span class="text-[11px] font-medium text-slate-500 dark:text-slate-400 flex items-center gap-1 mr-1">
					<Scissors class="w-3 h-3 text-rose-500" />
					<span>Links (Click to Sever/Restore):</span>
				</span>
				{#each renderedLinks as link (link.key)}
					<button
						type="button"
						onclick={() => handleToggleLink(link.srcFp, link.dstFp)}
						class="inline-flex items-center gap-1.5 px-2 py-1 rounded-md text-[11px] font-mono transition-all border {link.isSevered
							? 'bg-rose-50 dark:bg-rose-950/40 text-rose-700 dark:text-rose-400 border-rose-300 dark:border-rose-800 hover:bg-rose-100'
							: 'bg-slate-50 dark:bg-slate-800/60 text-slate-700 dark:text-slate-300 border-slate-200 dark:border-slate-700 hover:border-indigo-400 dark:hover:border-indigo-600'}"
					>
						<span>{shortenFingerprint(link.srcFp)} ↔ {shortenFingerprint(link.dstFp)}</span>
						{#if link.isSevered}
							<span class="text-[9px] font-semibold text-rose-600 dark:text-rose-400 uppercase tracking-wider">Severed</span>
						{:else}
							<span class="text-[10px] text-indigo-600 dark:text-indigo-400 font-semibold">{link.rttMs}ms</span>
						{/if}
					</button>
				{/each}
			</div>
		{/if}
	</header>

	<!-- Main Workspace: Interactive SVG Canvas (Left) + Inspector Panel (Right) -->
	<div class="flex-1 min-h-0 flex flex-col lg:flex-row overflow-hidden">
		<!-- Interactive SVG Canvas Area -->
		<div class="flex-1 relative bg-radial from-slate-100 via-slate-50 to-slate-100 dark:from-slate-900/50 dark:via-slate-950 dark:to-slate-900/60 overflow-hidden select-none">
			<!-- svelte-ignore a11y_no_static_element_interactions -->
			<svg
				class="w-full h-full cursor-crosshair touch-none"
				viewBox="0 0 {svgWidth} {svgHeight}"
				role="presentation"
				onpointermove={handleSvgPointerMove}
				onpointerup={handleSvgPointerUp}
			>
				<defs>
					<!-- Linear gradients for nodes -->
					<linearGradient id="localGrad" x1="0" y1="0" x2="1" y2="1">
						<stop offset="0%" stop-color="#6366F1" />
						<stop offset="100%" stop-color="#4F46E5" />
					</linearGradient>

					<linearGradient id="relayGrad" x1="0" y1="0" x2="1" y2="1">
						<stop offset="0%" stop-color="#F59E0B" />
						<stop offset="100%" stop-color="#D97706" />
					</linearGradient>

					<linearGradient id="peerGrad" x1="0" y1="0" x2="1" y2="1">
						<stop offset="0%" stop-color="#0EA5E9" />
						<stop offset="100%" stop-color="#0284C7" />
					</linearGradient>

					<!-- Drop Shadow Filter -->
					<filter id="nodeShadow" x="-20%" y="-20%" width="140%" height="140%">
						<feDropShadow dx="0" dy="4" stdDeviation="6" flood-opacity="0.15" />
					</filter>
				</defs>

				<!-- Links / Edges -->
				{#each renderedLinks as link (link.key)}
					{@const p1 = getNodePos(link.srcFp)}
					{@const p2 = getNodePos(link.dstFp)}
					{@const midX = (p1.x + p2.x) / 2}
					{@const midY = (p1.y + p2.y) / 2}
					{@const isOnActiveRoute =
						dijkstraRoute?.reachable &&
						dijkstraRoute.pathFps.includes(link.srcFp.toUpperCase()) &&
						dijkstraRoute.pathFps.includes(link.dstFp.toUpperCase())}

					<g class="transition-all">
						<!-- Background wide path for easy clicking to toggle sever -->
						<!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
						<line
							x1={p1.x}
							y1={p1.y}
							x2={p2.x}
							y2={p2.y}
							stroke="transparent"
							stroke-width="20"
							class="cursor-pointer"
							onclick={() => handleToggleLink(link.srcFp, link.dstFp)}
						/>

						<!-- Visible Edge -->
						<line
							x1={p1.x}
							y1={p1.y}
							x2={p2.x}
							y2={p2.y}
							class="transition-all duration-300"
							stroke={link.isSevered
								? '#EF4444'
								: isOnActiveRoute
									? '#6366F1'
									: '#94A3B8'}
							stroke-width={isOnActiveRoute ? '3.5' : '2'}
							stroke-dasharray={link.isSevered ? '6,6' : 'none'}
							stroke-opacity={link.isSevered ? '0.9' : isOnActiveRoute ? '1.0' : '0.6'}
						/>

						<!-- Midpoint Badge showing latency or severed state -->
						<!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
						<g
							transform="translate({midX}, {midY})"
							class="cursor-pointer group"
							role="button"
							tabindex="0"
							onclick={() => handleToggleLink(link.srcFp, link.dstFp)}
							onkeydown={(e) => {
								if (e.key === 'Enter' || e.key === ' ') handleToggleLink(link.srcFp, link.dstFp);
							}}
						>
							{#if link.isSevered}
								<rect
									x="-24"
									y="-10"
									width="48"
									height="20"
									rx="6"
									class="fill-rose-500 stroke-rose-600 transition-colors"
								/>
								<text
									text-anchor="middle"
									y="4"
									class="fill-white text-[9px] font-mono font-bold select-none uppercase tracking-wider"
								>
									CUT
								</text>
							{:else}
								<rect
									x="-20"
									y="-9"
									width="40"
									height="18"
									rx="5"
									class="fill-white dark:fill-slate-800 stroke-slate-200 dark:stroke-slate-700 group-hover:stroke-indigo-400 transition-colors"
								/>
								<text
									text-anchor="middle"
									y="3.5"
									class="fill-slate-700 dark:fill-slate-300 text-[10px] font-mono font-semibold select-none group-hover:fill-indigo-600"
								>
									{link.rttMs}ms
								</text>
							{/if}
						</g>
					</g>
				{/each}

				<!-- Nodes -->
				{#if appState.meshTopology}
					{#each appState.meshTopology.nodes as node (node.fingerprint)}
						{@const pos = getNodePos(node.fingerprint)}
						{@const isLocal =
							node.fingerprint.toUpperCase() ===
							appState.meshTopology.localFingerprint.toUpperCase()}
						{@const isSelected =
							selectedNode?.fingerprint.toUpperCase() === node.fingerprint.toUpperCase()}
						{@const isRelay = node.relayEnabled && !isLocal}

						<!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
						<g
							transform="translate({pos.x}, {pos.y})"
							class="cursor-grab active:cursor-grabbing group"
							role="button"
							tabindex="0"
							onpointerdown={(e) => handleNodePointerDown(e, node.fingerprint)}
							onclick={() => (appState.selectedMeshNodeFp = node.fingerprint)}
							onkeydown={(e) => {
								if (e.key === 'Enter' || e.key === ' ') appState.selectedMeshNodeFp = node.fingerprint;
							}}
						>
							<!-- Selected Node Pulsing Halo -->
							{#if isSelected}
								<circle
									r="38"
									fill="none"
									stroke="#6366F1"
									stroke-width="2"
									stroke-dasharray="6,4"
									opacity="0.8"
								/>
							{/if}

							<!-- Outer Circle -->
							<circle
								r="28"
								fill={isLocal
									? 'url(#localGrad)'
									: isRelay
										? 'url(#relayGrad)'
										: 'url(#peerGrad)'}
								stroke="#FFFFFF"
								stroke-width="2.5"
								filter="url(#nodeShadow)"
								class="transition-transform duration-150 group-hover:scale-105"
							/>

							<!-- Node Glyph Icon in Center -->
							<text
								text-anchor="middle"
								dominant-baseline="central"
								class="fill-white font-bold text-xs select-none pointer-events-none"
							>
								{#if isLocal}
									★
								{:else if isRelay}
									⇄
								{:else}
									●
								{/if}
							</text>

							<!-- Relay Badge -->
							{#if isRelay}
								<g transform="translate(0, -34)">
									<rect
										x="-20"
										y="-8"
										width="40"
										height="16"
										rx="8"
										fill="#F59E0B"
									/>
									<text
										text-anchor="middle"
										y="3"
										class="fill-white text-[9px] font-bold font-mono uppercase tracking-wider select-none pointer-events-none"
									>
										RELAY
									</text>
								</g>
							{/if}

							<!-- Node Label (Alias & Subtitle) -->
							<text
								text-anchor="middle"
								y="44"
								class="fill-slate-900 dark:fill-slate-100 text-[12px] font-bold select-none pointer-events-none"
							>
								{node.alias}
							</text>
							<text
								text-anchor="middle"
								y="57"
								class="fill-slate-500 dark:fill-slate-400 text-[10px] font-mono select-none pointer-events-none"
							>
								{isLocal ? '(This Device)' : shortenFingerprint(node.fingerprint)}
							</text>
						</g>
					{/each}
				{/if}
			</svg>

			<!-- Canvas Overlay Guide / Legend -->
			<div
				class="absolute bottom-3 left-3 bg-white/90 dark:bg-slate-900/90 backdrop-blur-xs p-2.5 rounded-xl border border-slate-200 dark:border-slate-800 shadow-sm text-[11px] space-y-1.5 pointer-events-none"
			>
				<div class="font-semibold text-slate-800 dark:text-slate-200 text-xs">
					Topology Legend
				</div>
				<div class="flex items-center gap-2 text-slate-600 dark:text-slate-400">
					<span class="w-3 h-3 rounded-full bg-indigo-600 inline-block"></span>
					<span>Local Device</span>
				</div>
				<div class="flex items-center gap-2 text-slate-600 dark:text-slate-400">
					<span class="w-3 h-3 rounded-full bg-amber-500 inline-block"></span>
					<span>Cross-Subnet Relay</span>
				</div>
				<div class="flex items-center gap-2 text-slate-600 dark:text-slate-400">
					<span class="w-3 h-3 rounded-full bg-sky-500 inline-block"></span>
					<span>End Host Peer</span>
				</div>
				<div class="flex items-center gap-2 text-slate-600 dark:text-slate-400">
					<span class="w-4 h-0.5 border-t border-rose-500 border-dashed inline-block"></span>
					<span>Severed Link (Cut)</span>
				</div>
			</div>
		</div>

		<!-- Route Inspector & Diagnostics Panel (Right Side) -->
		<aside
			class="w-full lg:w-96 flex-none bg-white dark:bg-slate-900 border-t lg:border-t-0 lg:border-l border-slate-200 dark:border-slate-800 flex flex-col overflow-y-auto"
		>
			{#if selectedNode}
				<div class="p-4 space-y-4">
					<!-- Selected Node Header Card -->
					<div class="space-y-2">
						<div class="flex items-start justify-between gap-2">
							<div>
								<div class="flex items-center gap-2">
									<h3 class="text-base font-bold text-slate-900 dark:text-white">
										{selectedNode.alias}
									</h3>
									{#if selectedNode.fingerprint.toUpperCase() === appState.meshTopology?.localFingerprint.toUpperCase()}
										<span class="px-1.5 py-0.5 rounded text-[10px] font-mono bg-indigo-100 dark:bg-indigo-950 text-indigo-700 dark:text-indigo-300 font-semibold">
											Local Node
										</span>
									{:else if selectedNode.relayEnabled}
										<span class="px-1.5 py-0.5 rounded text-[10px] font-mono bg-amber-100 dark:bg-amber-950 text-amber-700 dark:text-amber-300 font-semibold">
											Active Relay
										</span>
									{:else}
										<span class="px-1.5 py-0.5 rounded text-[10px] font-mono bg-slate-100 dark:bg-slate-800 text-slate-600 dark:text-slate-400">
											Host Only
										</span>
									{/if}
								</div>
								<div class="flex items-center gap-1.5 mt-1">
									<span class="text-xs font-mono text-slate-500 dark:text-slate-400">
										{shortenFingerprint(selectedNode.fingerprint)}
									</span>
									<button
										type="button"
										onclick={() => handleCopyFp(selectedNode?.fingerprint || '')}
										class="p-0.5 rounded text-slate-400 hover:text-indigo-600 transition-colors"
										title="Copy Fingerprint"
									>
										{#if copiedFp === selectedNode.fingerprint}
											<Check class="w-3.5 h-3.5 text-emerald-500" />
										{:else}
											<Copy class="w-3.5 h-3.5" />
										{/if}
									</button>
								</div>
							</div>

							{#if selectedNode.fingerprint.toUpperCase() !== appState.meshTopology?.localFingerprint.toUpperCase()}
								<button
									onclick={() => handleSendFiles(selectedNode!)}
									class="flex items-center gap-1 px-2.5 py-1.5 rounded-lg bg-indigo-600 hover:bg-indigo-500 text-white text-xs font-medium transition-all shadow-xs"
									title="Transfer files to peer"
								>
									<Send class="w-3.5 h-3.5" />
									<span>Send</span>
								</button>
							{/if}
						</div>
					</div>

					<!-- Dijkstra Shortest Path Card -->
					<div class="p-3.5 rounded-xl bg-slate-50 dark:bg-slate-800/60 border border-slate-200 dark:border-slate-700/80 space-y-2.5">
						<div class="flex items-center justify-between">
							<span class="text-xs font-bold text-slate-900 dark:text-slate-100 flex items-center gap-1.5">
								<Route class="w-4 h-4 text-indigo-600 dark:text-indigo-400" />
								<span>Dijkstra Shortest Path</span>
							</span>
							{#if dijkstraRoute?.reachable}
								<span class="text-[11px] font-mono font-semibold text-emerald-600 dark:text-emerald-400">
									{dijkstraRoute.hops} {dijkstraRoute.hops === 1 ? 'Hop' : 'Hops'} • {dijkstraRoute.totalLatencyMs}ms
								</span>
							{:else}
								<span class="text-[11px] font-mono font-semibold text-rose-500">
									Unreachable
								</span>
							{/if}
						</div>

						{#if dijkstraRoute?.reachable}
							<div class="flex flex-wrap items-center gap-1.5 py-1">
								{#each dijkstraRoute.pathAliases as alias, idx (idx)}
									<span class="px-2 py-0.5 rounded-md text-xs font-medium {idx === 0
										? 'bg-indigo-100 dark:bg-indigo-950 text-indigo-700 dark:text-indigo-300 font-semibold'
										: idx === dijkstraRoute.pathAliases.length - 1
											? 'bg-sky-100 dark:bg-sky-950 text-sky-700 dark:text-sky-300 font-semibold'
											: 'bg-amber-100 dark:bg-amber-950 text-amber-700 dark:text-amber-300'}">
										{alias}
									</span>
									{#if idx < dijkstraRoute.pathAliases.length - 1}
										<span class="text-slate-400 dark:text-slate-500 text-xs font-bold">→</span>
									{/if}
								{/each}
							</div>
						{:else}
							<p class="text-xs text-rose-600 dark:text-rose-400 flex items-center gap-1">
								<AlertTriangle class="w-3.5 h-3.5" />
								<span>All available links to this node are severed or disconnected.</span>
							</p>
						{/if}
					</div>

					<!-- Multi-Hop Diagnostic Triggers -->
					{#if selectedNode.fingerprint.toUpperCase() !== appState.meshTopology?.localFingerprint.toUpperCase()}
						<div class="space-y-3 pt-2 border-t border-slate-200 dark:border-slate-800">
							<div class="flex items-center justify-between">
								<span class="text-xs font-bold text-slate-800 dark:text-slate-200">
									Overlay Diagnostics
								</span>
								{#if appState.diagnosticError}
									<span class="text-[10px] text-rose-500 truncate max-w-[180px]" title={appState.diagnosticError}>
										{appState.diagnosticError}
									</span>
								{/if}
							</div>

							<div class="grid grid-cols-2 gap-2">
								<button
									onclick={handleRunTraceroute}
									disabled={appState.isTracing}
									class="flex items-center justify-center gap-1.5 p-2 rounded-lg bg-slate-100 dark:bg-slate-800 hover:bg-slate-200 dark:hover:bg-slate-750 text-slate-800 dark:text-slate-200 border border-slate-200 dark:border-slate-700 text-xs font-medium transition-colors cursor-pointer disabled:opacity-50"
								>
									<Route class="w-3.5 h-3.5 text-indigo-600 dark:text-indigo-400 {appState.isTracing ? 'animate-spin' : ''}" />
									<span>{appState.isTracing ? 'Tracing...' : 'Run Traceroute'}</span>
								</button>

								<button
									onclick={handleRunBenchmark}
									disabled={appState.isBenchmarking}
									class="flex items-center justify-center gap-1.5 p-2 rounded-lg bg-slate-100 dark:bg-slate-800 hover:bg-slate-200 dark:hover:bg-slate-750 text-slate-800 dark:text-slate-200 border border-slate-200 dark:border-slate-700 text-xs font-medium transition-colors cursor-pointer disabled:opacity-50"
								>
									<Gauge class="w-3.5 h-3.5 text-emerald-600 dark:text-emerald-400 {appState.isBenchmarking ? 'animate-spin' : ''}" />
									<span>{appState.isBenchmarking ? 'Testing...' : 'Run Speedtest'}</span>
								</button>
							</div>

							<!-- Traceroute Hop Progression Card -->
							{#if appState.tracerouteResult}
								<div class="p-3 rounded-xl bg-slate-50 dark:bg-slate-800/80 border border-slate-200 dark:border-slate-700 space-y-2">
									<div class="flex items-center justify-between text-xs">
										<span class="font-bold text-slate-800 dark:text-slate-200 flex items-center gap-1.5">
											{#if appState.tracerouteResult.success}
												<CheckCircle2 class="w-3.5 h-3.5 text-emerald-500" />
												<span>Traceroute Reached Target</span>
											{:else}
												<XCircle class="w-3.5 h-3.5 text-rose-500" />
												<span>Traceroute Incomplete</span>
											{/if}
										</span>
										<span class="font-mono text-[11px] font-semibold text-indigo-600 dark:text-indigo-400">
											Total {appState.tracerouteResult.totalRttMs}ms
										</span>
									</div>

									<div class="space-y-1.5 pt-1">
										{#each appState.tracerouteResult.hops as hop, idx (idx)}
											<div class="flex items-center justify-between p-1.5 rounded-lg bg-white dark:bg-slate-900 border border-slate-200/80 dark:border-slate-750 text-xs">
												<div class="flex items-center gap-2">
													<span class="w-4 h-4 rounded-full bg-indigo-50 dark:bg-indigo-950 text-indigo-600 dark:text-indigo-400 text-[10px] font-bold flex items-center justify-center font-mono">
														{idx + 1}
													</span>
													<span class="font-medium text-slate-800 dark:text-slate-200">
														{hop.alias}
													</span>
													<span class="text-[10px] font-mono text-slate-400">
														({shortenFingerprint(hop.nodeFp)})
													</span>
												</div>
												<span class="font-mono font-semibold text-emerald-600 dark:text-emerald-400 text-xs">
													+{hop.rttMs}ms
												</span>
											</div>
										{/each}
									</div>
								</div>
							{/if}

							<!-- Speedtest Bandwidth Gauge Card -->
							{#if appState.speedtestResult}
								<div class="p-3.5 rounded-xl bg-gradient-to-br from-indigo-50 to-emerald-50 dark:from-slate-800 dark:to-slate-850 border border-indigo-200 dark:border-slate-700 space-y-3">
									<div class="flex items-center justify-between">
										<span class="text-xs font-bold text-slate-800 dark:text-slate-200 flex items-center gap-1.5">
											<Zap class="w-4 h-4 text-emerald-500" />
											<span>In-Memory Throughput</span>
										</span>
										<span class="text-[10px] font-mono px-1.5 py-0.5 rounded bg-emerald-100 dark:bg-emerald-950 text-emerald-700 dark:text-emerald-300 font-semibold">
											0 Disk I/O
										</span>
									</div>

									<!-- Throughput Display -->
									<div class="flex items-baseline gap-2">
										<span class="text-2xl font-black font-mono tracking-tight text-slate-900 dark:text-white">
											{appState.speedtestResult.throughputMbps.toFixed(1)}
										</span>
										<span class="text-xs font-bold text-emerald-600 dark:text-emerald-400 font-mono">
											Mbps
										</span>
										<span class="text-xs font-mono text-slate-500 dark:text-slate-400 ml-auto">
											({(appState.speedtestResult.throughputMbps / 8).toFixed(1)} MB/s)
										</span>
									</div>

									<!-- Metrics Grid -->
									<div class="grid grid-cols-3 gap-1.5 text-center pt-1 border-t border-slate-200/60 dark:border-slate-700/60">
										<div class="p-1.5 rounded-lg bg-white/80 dark:bg-slate-900/80">
											<div class="text-[10px] text-slate-500">Duration</div>
											<div class="font-mono font-semibold text-xs text-slate-800 dark:text-slate-200">
												{appState.speedtestResult.durationMs}ms
											</div>
										</div>
										<div class="p-1.5 rounded-lg bg-white/80 dark:bg-slate-900/80">
											<div class="text-[10px] text-slate-500">Transferred</div>
											<div class="font-mono font-semibold text-xs text-slate-800 dark:text-slate-200">
												{formatBytes(appState.speedtestResult.totalBytes)}
											</div>
										</div>
										<div class="p-1.5 rounded-lg bg-white/80 dark:bg-slate-900/80">
											<div class="text-[10px] text-slate-500">Chunks</div>
											<div class="font-mono font-semibold text-xs text-slate-800 dark:text-slate-200">
												{appState.speedtestResult.chunksReceived}
											</div>
										</div>
									</div>
								</div>
							{/if}
						</div>
					{/if}
				</div>
			{:else}
				<div class="flex-1 flex flex-col items-center justify-center p-6 text-center text-slate-400">
					<Network class="w-8 h-8 stroke-[1.5] mb-2 opacity-50" />
					<p class="text-xs">Select any node on the topology canvas to inspect route paths and run diagnostics</p>
				</div>
			{/if}
		</aside>
	</div>
</div>
