export const formatClock = (date: Date): string => {
  const jst = new Date(date.getTime() + 9 * 3600_000);
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${pad(jst.getUTCHours())}:${pad(jst.getUTCMinutes())}`;
};

export const formatDateTime = (date: Date): string => {
  const jst = new Date(date.getTime() + 9 * 3600_000);
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${jst.getUTCFullYear()}-${pad(jst.getUTCMonth() + 1)}-${pad(jst.getUTCDate())} ${pad(jst.getUTCHours())}:${pad(jst.getUTCMinutes())}`;
};

const MODEL_LABELS: Readonly<Record<string, string>> = {
  "opus": "Opus 級",
  "glm-5.3": "GLM 5.3",
  "deepseek-v4.1-flash": "DeepSeek V4.1 Flash",
};

export const formatModel = (model: string): string => MODEL_LABELS[model] ?? model;

export const formatDuration = (seconds: number): string => {
  const whole = Math.max(0, Math.round(seconds));
  if (whole < 60) {
    return `${whole}秒`;
  }
  const minutes = Math.floor(whole / 60);
  if (minutes < 60) {
    return `${minutes}分`;
  }
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  return rest === 0 ? `${hours}時間` : `${hours}時間${rest}分`;
};
