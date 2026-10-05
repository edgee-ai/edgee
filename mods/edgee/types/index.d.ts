export type EdgeeReroute = { model: string; expiresAt: number };

export type EdgeeRequest = {
  at: number;
  durationMs: number;
  requested: string;
  served: string | null;
  input: number;
  cached: number;
  output: number;
  subagent: boolean;
};

export type EdgeePicker = {
  filter?: string;
  notice?: string;
  failed?: boolean;
  pending?: string; // the model a pick is applying (OFF to clear), until the gateway answers
  frame?: number; // the pending spinner's frame
};

export type EdgeeModelTotals = { requests: number; input: number; cached: number; output: number };

declare module "claude-code" {
  interface PluginState {
    "edgee": {
      reroute: EdgeeReroute | null;
      requests: EdgeeRequest[];
      models: Record<string, EdgeeModelTotals>;
      catalog: string[];
      picker: EdgeePicker;
      minimized: boolean;
    };
  }
}
