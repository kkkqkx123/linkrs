import { call, client } from '$lib/api/client';

/**
 * Schema version inspection: history, change ranges and breaking-change
 * detection for a vertex tag or edge type.
 */
export const schemaVersionsService = {
	history: async (
		space: string,
		label: string,
		isEdge = false,
	): Promise<unknown> =>
		call(
			client.GET('/v1/schema/versions/{space}/{label}', {
				params: { path: { space, label }, query: { is_edge: isEdge } },
			}),
		),

	changes: async (
		space: string,
		label: string,
		fromVersion: number,
		toVersion: number,
		isEdge = false,
	): Promise<unknown> =>
		call(
			client.GET('/v1/schema/changes/{space}/{label}/{from_version}/{to_version}', {
				params: {
					path: {
						space,
						label,
						from_version: fromVersion,
						to_version: toVersion,
					},
					query: { is_edge: isEdge },
				},
			}),
		),

	breakingChanges: async (
		space: string,
		label: string,
		fromVersion: number,
		toVersion: number,
		isEdge = false,
	): Promise<unknown> =>
		call(
			client.GET(
				'/v1/schema/breaking-changes/{space}/{label}/{from_version}/{to_version}',
				{
					params: {
						path: {
							space,
							label,
							from_version: fromVersion,
							to_version: toVersion,
						},
						query: { is_edge: isEdge },
					},
				},
			),
		),
};

export default schemaVersionsService;
