export interface PaletteCommand {
	id: string;
	/** Resolved display text; i18n must be read lazily so the active
	 * language applies without rebuilding the command list. */
	label: () => string;
	keywords?: () => string;
	run: () => void;
}

/** Initials of each word in the label, e.g. "Data Browser" -> "db". */
function initials(label: string): string {
	return label
		.split(/\s+/)
		.map((word) => word.charAt(0))
		.join('');
}

/**
 * Filter palette commands by a free-text query. Matching is
 * case-insensitive on: label substring, keyword substring, or the label's
 * word initials prefix ("db" matches "Data Browser"). An empty query
 * returns every command in the original order.
 */
export function filterCommands(
	commands: PaletteCommand[],
	query: string,
): PaletteCommand[] {
	const q = query.trim().toLowerCase();
	if (!q) return commands;
	return commands.filter((cmd) => {
		const label = cmd.label().toLowerCase();
		if (label.includes(q)) return true;
		if (cmd.keywords && cmd.keywords().toLowerCase().includes(q)) return true;
		return initials(label).startsWith(q);
	});
}
