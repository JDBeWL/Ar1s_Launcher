/** 日志级别解析与筛选：无级别行跟随上一条日志 */
export type LogLevel = 'fatal' | 'error' | 'warn' | 'info' | 'debug' | 'trace';
export type LogFilter = 'all' | 'warn' | 'error';

export interface ParsedLogLine {
  text: string;
  level: LogLevel | null;
  ownerLevel: LogLevel | null;
  lineNo: number;
}

const LEVEL_RE = /\[[^\]]*\/(FATAL|ERROR|WARN|INFO|DEBUG|TRACE)\]/i;
const LEVEL_ONLY_RE = /\[(FATAL|ERROR|WARN|INFO|DEBUG|TRACE)\]/i;

const RANK: Record<LogLevel, number> = {
  trace: 0,
  debug: 1,
  info: 2,
  warn: 3,
  error: 4,
  fatal: 5,
};

const THRESHOLD: Record<LogFilter, number> = {
  all: -1,
  warn: 3,
  error: 4,
};

export function parseLogLines(lines: string[]): ParsedLogLine[] {
  let current: LogLevel | null = null;

  return lines.map((text, index) => {
    const match = LEVEL_RE.exec(text) ?? LEVEL_ONLY_RE.exec(text);
    const level = match ? (match[1].toLowerCase() as LogLevel) : null;
    if (level) current = level;
    return {
      text,
      level,
      ownerLevel: level ?? current,
      lineNo: index + 1,
    };
  });
}

export function filterLogLines(parsed: ParsedLogLine[], filter: LogFilter, query: string): ParsedLogLine[] {
  const threshold = THRESHOLD[filter];
  const needle = query.trim().toLowerCase();

  return parsed.filter((line) => {
    if (threshold >= 0) {
      const owner = line.ownerLevel;
      if (!owner || RANK[owner] < threshold) return false;
    }
    return !needle || line.text.toLowerCase().includes(needle);
  });
}
