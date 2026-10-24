export function formatCypherValue(value: unknown): string {
	if (value === null || value === undefined) return 'NULL';
	if (typeof value === 'number' && Number.isFinite(value)) return String(value);
	if (typeof value === 'boolean') return value ? 'true' : 'false';
	if (typeof value === 'string') {
		const numeric = value.trim() !== '' && Number.isFinite(Number(value));
		if (numeric) return value;
		return `"${value.replace(/\\/g, '\\\\').replace(/"/g, '\\"')}"`;
	}
	return `"${JSON.stringify(value).replace(/\\/g, '\\\\').replace(/"/g, '\\"')}"`;
}

export function quoteVid(vid: string): string {
	if (vid.trim() !== '' && Number.isFinite(Number(vid))) return vid;
	return formatCypherValue(vid);
}

export function buildVertexUpdate(
	vid: string,
	properties: Record<string, unknown>,
): string {
	const assignments = Object.entries(properties).map(
		([key, value]) => `${key} = ${formatCypherValue(value)}`,
	);
	if (assignments.length === 0) return `UPDATE VERTEX ${quoteVid(vid)} SET `;
	return `UPDATE VERTEX ${quoteVid(vid)} SET ${assignments.join(', ')}`;
}

export function buildVertexDelete(vid: string): string {
	return `DELETE VERTEX ${quoteVid(vid)} WITH EDGE`;
}

export function buildEdgeUpdate(
	edgeType: string,
	src: string,
	dst: string,
	rank: number,
	properties: Record<string, unknown>,
): string {
	const assignments = Object.entries(properties).map(
		([key, value]) => `${key} = ${formatCypherValue(value)}`,
	);
	const head = `UPDATE EDGE ${quoteVid(src)} -> ${quoteVid(dst)} @${rank} OF ${edgeType}`;
	if (assignments.length === 0) return `${head} SET `;
	return `${head} SET ${assignments.join(', ')}`;
}

export function buildEdgeDelete(
	edgeType: string,
	src: string,
	dst: string,
	rank: number,
): string {
	return `DELETE EDGE ${quoteVid(src)} -> ${quoteVid(dst)} @${rank} OF ${edgeType}`;
}
