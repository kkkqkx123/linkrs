import { call, client } from '$lib/api/client';

export interface RegisterFunctionParams {
	name: string;
	implementation: string;
	function_type?: string;
	parameters?: string[];
	return_type?: string;
	description?: string;
}

/**
 * Custom function registry management. Only library-backed UDFs can
 * execute, so registration requires the local dynamic-library path;
 * metadata-only fields ride along for client compatibility.
 */
export const functionsService = {
	list: async (): Promise<unknown> => call(client.GET('/v1/functions')),

	info: async (name: string): Promise<unknown> =>
		call(
			client.GET('/v1/functions/{name}', {
				params: { path: { name } },
			}),
		),

	register: async (params: RegisterFunctionParams): Promise<unknown> =>
		call(
			client.POST('/v1/functions', {
				body: {
					name: params.name,
					implementation: params.implementation,
					type: params.function_type ?? 'custom',
					parameters: params.parameters ?? [],
					return_type: params.return_type ?? '',
					description: params.description ?? '',
				} as never,
			}),
		),

	unregister: async (name: string): Promise<unknown> =>
		call(
			client.DELETE('/v1/functions/{name}', {
				params: { path: { name } },
			}),
		),
};

export default functionsService;
