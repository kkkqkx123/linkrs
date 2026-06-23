import { get } from "@/utils/http";
import type {
  VertexListResponse,
  EdgeListResponse,
  FilterGroup,
  Statistics,
} from "@/types/dataBrowser";

export interface DataBrowserService {
  getVertices: (
    space: string,
    tag: string,
    page: number,
    pageSize: number,
    sort: { field: string; order: "asc" | "desc" },
    filters: FilterGroup,
  ) => Promise<VertexListResponse>;

  getEdges: (
    space: string,
    type: string,
    page: number,
    pageSize: number,
    sort: { field: string; order: "asc" | "desc" },
    filters: FilterGroup,
  ) => Promise<EdgeListResponse>;

  getStatistics: (space: string) => Promise<Statistics>;
}

export const dataBrowserService: DataBrowserService = {
  getVertices: async (space, tag, page, pageSize, sort, filters) => {
    const params: Record<string, string | number> = {
      limit: pageSize,
      offset: (page - 1) * pageSize,
      sort_by: sort.field,
      sort_order: sort.order.toUpperCase(),
    };

    if (filters && filters.conditions.length > 0) {
      params.filter = JSON.stringify(filters);
    }

    const response = (await get(`/api/spaces/${space}/tags/${tag}/vertices`)(
      params,
    )) as VertexListResponse;
    return response;
  },

  getEdges: async (space, type, page, pageSize, sort, filters) => {
    const params: Record<string, string | number> = {
      limit: pageSize,
      offset: (page - 1) * pageSize,
      sort_by: sort.field,
      sort_order: sort.order.toUpperCase(),
    };

    if (filters && filters.conditions.length > 0) {
      params.filter = JSON.stringify(filters);
    }

    const response = (await get(
      `/api/spaces/${space}/edge-types/${type}/edges`,
    )(params)) as EdgeListResponse;
    return response;
  },

  getStatistics: async (space) => {
    const response = (await get("/api/data/statistics")({
      space,
    })) as Statistics;
    return response;
  },
};
