import { call, client } from '$lib/api/client';
import {
	streamMigrationProgress,
	type MigrationProgressEvent,
	type MigrationStreamOutcome,
} from './migrationStream';

export type { MigrationProgressEvent, MigrationStreamOutcome };

export interface MigrationPlanParams {
	space: string;
	label: string;
	from_version: number;
	to_version: number;
	is_edge?: boolean;
	expand_contract?: boolean;
}

/**
 * Schema migration management: plan, dry-run, execute, rollback,
 * history and status. Progress streams over SSE (`GET
 * /v1/migration/stream/{space}/{label}`), which OpenAPI cannot model,
 * so it uses raw fetch decoded into typed events (see migrationStream).
 */
export const migrationService = {
	plan: async (params: MigrationPlanParams): Promise<unknown> =>
		call(
			client.POST('/v1/migration/plan/{space}/{label}', {
				params: {
					path: { space: params.space, label: params.label },
					query: {
						from_version: params.from_version,
						to_version: params.to_version,
						is_edge: params.is_edge ?? false,
						expand_contract: params.expand_contract ?? false,
					},
				},
			}),
		),

	execute: async (planJson: string): Promise<unknown> =>
		call(
			client.POST('/v1/migration/execute', {
				body: { plan_json: planJson },
			}),
		),

	rollback: async (planJson: string): Promise<unknown> =>
		call(
			client.POST('/v1/migration/rollback', {
				body: { plan_json: planJson },
			}),
		),

	dryRun: async (planJson: string): Promise<unknown> =>
		call(
			client.POST('/v1/migration/dry-run', {
				body: { plan_json: planJson },
			}),
		),

	history: async (
		space: string,
		label: string,
		isEdge = false,
	): Promise<unknown> =>
		call(
			client.GET('/v1/migration/history/{space}/{label}', {
				params: { path: { space, label }, query: { is_edge: isEdge } },
			}),
		),

	status: async (
		space: string,
		label: string,
		isEdge = false,
	): Promise<unknown> =>
		call(
			client.GET('/v1/migration/status/{space}/{label}', {
				params: { path: { space, label }, query: { is_edge: isEdge } },
			}),
		),

	streamProgress: async (
		space: string,
		label: string,
		options: {
			isEdge?: boolean;
			signal?: AbortSignal;
			onEvent?: (event: MigrationProgressEvent) => void;
		} = {},
	): Promise<MigrationStreamOutcome> =>
		streamMigrationProgress(space, label, options),
};

export default migrationService;
