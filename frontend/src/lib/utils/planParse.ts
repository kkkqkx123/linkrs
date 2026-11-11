/**
 * Parsers for the ASCII execution plan emitted by `EXPLAIN` (table format).
 *
 * The backend renders the plan as a pipe-delimited table preceded by
 * free-text metadata lines:
 *
 *   Execution Model: Batch Pull
 *   Parallel Workers: requested=1
 *
 *   +------+------------------+------------+--- ...
 *   | id   | name             | deps       | --- ...
 *   +------+------------------+------------+--- ...
 *   |    1 | ScanVertices     | -          | --- ...
 *   +------+------------------+------------+--- ...
 *
 * The `deps` column lists the ids a node consumes, so the assembled tree
 * grows from the leaf nodes (no deps) towards their consumers. Cells wider
 * than the fixed column budget arrive cut short with a `...` suffix, and
 * plans may also come back as plain text or DOT, which these parsers reject
 * so callers can fall back to a raw text view.
 */

/** One row of the plan table, with deps already resolved to numbers. */
export interface PlanNode {
	id: number;
	name: string;
	deps: number[];
	profilingData: string;
	operatorInfo: string;
	outputVar: string;
}

/** A plan node with the consumers mounted below it. */
export interface PlanTreeNode extends PlanNode {
	children: PlanTreeNode[];
}

/** Column names of the plan table; `deps` may be `-` when a node has none. */
const REQUIRED_COLUMNS = ['id', 'name', 'deps'] as const;

/** Split a `|`-delimited row into trimmed cells, padding missing columns. */
function rowCells(line: string, width: number): string[] {
	const cells = line
		.split('|')
		.slice(1, -1)
		.map((cell) => cell.trim());
	while (cells.length < width) cells.push('');
	return cells;
}

/** Lowercased header cells of a `|`-delimited row. */
function headerCells(line: string): string[] {
	return line
		.split('|')
		.slice(1, -1)
		.map((cell) => cell.trim().toLowerCase());
}

/** Locate the header row of the plan table, or null when the text has none. */
function findHeaderRow(lines: string[]): number | null {
	for (let index = 0; index < lines.length; index++) {
		const line = lines[index];
		if (!line.trimStart().startsWith('|')) continue;
		const cells = headerCells(line);
		if (REQUIRED_COLUMNS.every((column) => cells.includes(column))) {
			return index;
		}
	}
	return null;
}

/** Parse the deps cell: `-`, blank or missing mean no dependencies. */
function parseDeps(cell: string): number[] {
	const trimmed = cell.trim();
	if (!trimmed || trimmed === '-') return [];
	return trimmed
		.split(/[,\s]+/)
		.filter((token) => token.length > 0)
		.map(Number)
		.filter((id) => Number.isInteger(id));
}

/** Read one cell by column index; absent optional columns yield ''. */
function cellAt(cells: string[], index: number | undefined): string {
	return index === undefined ? '' : (cells[index] ?? '');
}

/**
 * Parse the ASCII plan table into nodes.
 *
 * Returns null when the text is not a plan table (plain text, DOT or JSON
 * plans, arbitrary query output), so the caller can fall back to showing
 * the raw text instead.
 */
export function parsePlanTable(text: string): PlanNode[] | null {
	if (!text) return null;
	const lines = text.split(/\r?\n/);
	const headerIndex = findHeaderRow(lines);
	if (headerIndex === null) return null;

	const names = headerCells(lines[headerIndex]);
	const columnAt = new Map<string, number>();
	names.forEach((name, index) => {
		if (!columnAt.has(name)) columnAt.set(name, index);
	});
	const idIndex = columnAt.get('id');
	const nameIndex = columnAt.get('name');
	const depsIndex = columnAt.get('deps');
	if (
		idIndex === undefined ||
		nameIndex === undefined ||
		depsIndex === undefined
	) {
		return null;
	}

	const nodes: PlanNode[] = [];
	for (let index = headerIndex + 1; index < lines.length; index++) {
		const line = lines[index];
		const trimmed = line.trimStart();
		// Separator rows start with `+`; anything else ends the table.
		if (trimmed.startsWith('+')) continue;
		if (!trimmed.startsWith('|')) break;

		const cells = rowCells(line, names.length);
		const id = Number(cells[idIndex]);
		// Rows without a usable id carry no tree information, so they are
		// skipped instead of corrupting the node list.
		if (!Number.isInteger(id)) continue;

		nodes.push({
			id,
			name: cells[nameIndex] ?? '',
			deps: parseDeps(cells[depsIndex] ?? ''),
			profilingData: cellAt(cells, columnAt.get('profiling_data')),
			operatorInfo: cellAt(cells, columnAt.get('operator_info')),
			outputVar: cellAt(cells, columnAt.get('output_var')),
		});
	}
	return nodes;
}

/**
 * Assemble plan nodes into a consumer tree.
 *
 * Nodes without dependencies become roots. Every other node hangs below its
 * first resolvable dependency only, so a node feeding several consumers never
 * appears twice. Nodes whose deps reference unknown ids become extra roots
 * so no operator disappears from the view.
 */
export function buildPlanTree(nodes: PlanNode[]): PlanTreeNode[] {
	const byId = new Map<number, PlanTreeNode>();
	for (const node of nodes) {
		if (!byId.has(node.id)) {
			byId.set(node.id, { ...node, children: [] });
		}
	}

	const roots: PlanTreeNode[] = [];
	const mounted = new Set<number>();
	for (const node of nodes) {
		// Duplicate ids mount once, keeping the first occurrence's data.
		if (mounted.has(node.id)) continue;
		mounted.add(node.id);
		const treeNode = byId.get(node.id);
		if (!treeNode) continue;

		const parentId = node.deps.find((dep) => byId.has(dep));
		const parent = parentId === undefined ? undefined : byId.get(parentId);
		if (parent) {
			parent.children.push(treeNode);
		} else {
			roots.push(treeNode);
		}
	}
	return roots;
}
