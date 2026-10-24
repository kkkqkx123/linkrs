import { call, client } from '$lib/api/client';

export interface ActiveTransaction {
	transaction_id: number;
	state: string;
	owner: string;
	elapsed_ms: number;
	last_activity_ms: number;
	rollback_only: boolean;
	staged_bytes: number;
	undo_bytes: number;
	blocking_reason: unknown;
}

function toTransaction(row: Record<string, unknown>): ActiveTransaction {
	const num = (value: unknown): number =>
		typeof value === 'number' && Number.isFinite(value) ? value : 0;
	const str = (value: unknown): string =>
		typeof value === 'string' ? value : String(value ?? '');
	return {
		transaction_id: num(row.transaction_id),
		state: str(row.state),
		owner: str(row.owner),
		elapsed_ms: num(row.elapsed_ms),
		last_activity_ms: num(row.last_activity_ms),
		rollback_only: row.rollback_only === true,
		staged_bytes: num(row.staged_bytes),
		undo_bytes: num(row.undo_bytes),
		blocking_reason: row.blocking_reason ?? null,
	};
}

export const operationsService = {
	config: async (): Promise<unknown> => call(client.GET('/v1/config')),

	configKey: async (section: string, key: string): Promise<unknown> =>
		call(
			client.GET('/v1/config/{section}/{key}', {
				params: { path: { section, key } },
			}),
		),

	updateConfigKey: async (
		section: string,
		key: string,
		value: unknown,
	): Promise<unknown> =>
		call(
			client.PUT('/v1/config/{section}/{key}', {
				params: { path: { section, key } },
				body: { value: value as never },
			}),
		),

	resetConfigKey: async (section: string, key: string): Promise<unknown> =>
		call(
			client.DELETE('/v1/config/{section}/{key}', {
				params: { path: { section, key } },
			}),
		),

	transactions: async (): Promise<ActiveTransaction[]> => {
		const payload = await call<unknown>(client.GET('/v1/transactions'));
		if (!Array.isArray(payload)) return [];
		return payload
			.filter(
				(item): item is Record<string, unknown> =>
					typeof item === 'object' && item !== null,
			)
			.map(toTransaction);
	},

	killTransaction: async (id: number): Promise<void> => {
		await call(
			client.POST('/v1/transactions/{id}/kill', {
				params: { path: { id } },
			}),
		);
	},
};

export default operationsService;
