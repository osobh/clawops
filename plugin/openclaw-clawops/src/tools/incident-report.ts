/**
 * gf_incident_report — Generate or retrieve structured incident reports
 *
 * Triage uses this to produce incident reports after investigation.
 * Commander synthesizes these into human-readable summaries for the operator.
 * All incidents are stored in GatewayForge DB for Briefer's pattern recognition.
 */

import type { ClawOpsConfig, OpenClawTool } from '../index';
import axios from 'axios';

export interface IncidentReportParams {
  /** Create a new incident report */
  action: 'create' | 'get' | 'update' | 'list';
  /** Incident ID (for get/update) */
  incidentId?: string;
  /** New incident data (for create) */
  incident?: IncidentInput;
  /** Update fields (for update) */
  update?: Partial<IncidentInput>;
  /** List filters */
  listFilter?: {
    from?: string;
    to?: string;
    severity?: IncidentSeverity;
    status?: IncidentStatus;
    limit?: number;
  };
}

export type IncidentSeverity = 'sev1' | 'sev2' | 'sev3' | 'sev4';
export type IncidentStatus = 'DETECTING' | 'INVESTIGATING' | 'MITIGATING' | 'RESOLVED' | 'POST_MORTEM';

export interface IncidentInput {
  title: string;
  severity: IncidentSeverity;
  affectedAccounts: string[];
  affectedInstances: string[];
  affectedProviders: string[];
  affectedRegions: string[];
  detectedAt: string;
  description: string;
  symptoms: string[];
  timeline: TimelineEntry[];
  rootCause?: string;
  resolution?: string;
  actionItems?: ActionItem[];
  status: IncidentStatus;
}

export interface TimelineEntry {
  timestamp: string;
  event: string;
  agent: string;
  automated: boolean;
}

export interface ActionItem {
  description: string;
  owner: string;
  dueDate?: string;
  status: 'open' | 'in_progress' | 'done';
}

export interface IncidentRecord extends IncidentInput {
  incidentId: string;
  createdAt: string;
  updatedAt: string;
  resolvedAt: string | null;
  durationMinutes: number | null;
  userImpactMinutes: number | null;
  slaBreached: boolean;
}

export interface IncidentReportResult {
  action: string;
  incident?: IncidentRecord;
  incidents?: IncidentRecord[];
  total?: number;
}

export function incidentReportTool(config: ClawOpsConfig): OpenClawTool {
  return {
    name: 'gf_incident_report',
    description:
      'Create, retrieve, update, or list structured incident reports. ' +
      'Triage creates incident reports during investigation. ' +
      'Commander retrieves them to synthesize operator-facing summaries. ' +
      'Briefer queries incident history for daily/weekly reporting and pattern detection. ' +
      'Severity: sev1=all users impacted, sev2=significant subset, sev3=minor, sev4=cosmetic. ' +
      'Example: gf_incident_report({ action: "create", incident: { ' +
      'title: "Hetzner Nuremberg outage", severity: "sev2", ... } }) ' +
      'Example: gf_incident_report({ action: "list", listFilter: { from: "2024-01-01Z" } })',

    inputSchema: {
      type: 'object',
      properties: {
        action: {
          type: 'string',
          enum: ['create', 'get', 'update', 'list'],
          description: 'Operation to perform',
        },
        incidentId: {
          type: 'string',
          description: 'Incident ID (required for get/update)',
        },
        incident: {
          type: 'object',
          description: 'Incident data (required for create)',
          properties: {
            title: { type: 'string' },
            severity: { type: 'string', enum: ['sev1', 'sev2', 'sev3', 'sev4'] },
            affectedAccounts: { type: 'array', items: { type: 'string' } },
            affectedInstances: { type: 'array', items: { type: 'string' } },
            affectedProviders: { type: 'array', items: { type: 'string' } },
            affectedRegions: { type: 'array', items: { type: 'string' } },
            detectedAt: { type: 'string' },
            description: { type: 'string' },
            symptoms: { type: 'array', items: { type: 'string' } },
            status: { type: 'string' },
          },
        },
        update: {
          type: 'object',
          description: 'Fields to update on an existing incident',
        },
        listFilter: {
          type: 'object',
          properties: {
            from: { type: 'string' },
            to: { type: 'string' },
            severity: { type: 'string' },
            status: { type: 'string' },
            limit: { type: 'number' },
          },
        },
      },
      required: ['action'],
    },

    execute: async (params: unknown): Promise<IncidentReportResult> => {
      const p = params as IncidentReportParams;
      const headers = {
        Authorization: `Bearer ${config.gfApiKey}`,
        'Content-Type': 'application/json',
      };

      switch (p.action) {
        case 'create': {
          const response = await axios.post<IncidentRecord>(
            `${config.gfApiBase}/v1/incidents`,
            p.incident,
            { headers, timeout: 10_000 }
          );
          return { action: 'create', incident: response.data };
        }

        case 'get': {
          if (!p.incidentId) throw new Error('incidentId required for get action');
          const response = await axios.get<IncidentRecord>(
            `${config.gfApiBase}/v1/incidents/${encodeURIComponent(p.incidentId)}`,
            { headers, timeout: 10_000 }
          );
          return { action: 'get', incident: response.data };
        }

        case 'update': {
          if (!p.incidentId) throw new Error('incidentId required for update action');
          const response = await axios.patch<IncidentRecord>(
            `${config.gfApiBase}/v1/incidents/${encodeURIComponent(p.incidentId)}`,
            p.update,
            { headers, timeout: 10_000 }
          );
          return { action: 'update', incident: response.data };
        }

        case 'list': {
          const queryParams = new URLSearchParams();
          const f = p.listFilter ?? {};
          if (f.from) queryParams.set('from', f.from);
          if (f.to) queryParams.set('to', f.to);
          if (f.severity) queryParams.set('severity', f.severity);
          if (f.status) queryParams.set('status', f.status);
          if (f.limit) queryParams.set('limit', String(f.limit));

          const response = await axios.get<{ incidents: IncidentRecord[]; total: number }>(
            `${config.gfApiBase}/v1/incidents?${queryParams}`,
            { headers, timeout: 10_000 }
          );
          return {
            action: 'list',
            incidents: response.data.incidents,
            total: response.data.total,
          };
        }

        default:
          throw new Error(`Unknown action: ${p.action}`);
      }
    },
  };
}
