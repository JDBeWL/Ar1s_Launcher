import { describe, expect, it } from 'vitest';
import { filterLogLines, parseLogLines } from './logViewer';

const SAMPLE = [
  '[12:00:00] [main/INFO]: starting',
  '    at info.stack',
  '[12:00:01] [main/WARN]: careful',
  '    at warn.stack',
  '[12:00:02] [Render thread/ERROR]: boom',
  'java.lang.RuntimeException: failed',
  '    at error.stack',
  '[12:00:03] [main/DEBUG]: quietly',
];

describe('logViewer', () => {
  it('把无级别行归属到上一条日志', () => {
    const parsed = parseLogLines(SAMPLE);
    expect(parsed[1].ownerLevel).toBe('info');
    expect(parsed[3].ownerLevel).toBe('warn');
    expect(parsed[5].ownerLevel).toBe('error');
  });

  it('警告及以上保留警告和错误的堆栈', () => {
    expect(filterLogLines(parseLogLines(SAMPLE), 'warn', '').map((line) => line.lineNo)).toEqual([3, 4, 5, 6, 7]);
  });

  it('仅错误只保留错误和它的堆栈', () => {
    expect(filterLogLines(parseLogLines(SAMPLE), 'error', '').map((line) => line.lineNo)).toEqual([5, 6, 7]);
  });

  it('搜索与级别筛选同时生效', () => {
    expect(filterLogLines(parseLogLines(SAMPLE), 'warn', 'BOOM').map((line) => line.lineNo)).toEqual([5]);
  });

  it('兼容没有 logger 的 [main/ERROR] 写法', () => {
    const parsed = parseLogLines(['[12:00:00] [main/ERROR]: x']);
    expect(parsed[0].level).toBe('error');
  });

  it('全部筛选不隐藏未归类的行', () => {
    const parsed = parseLogLines(['before any level', '[12:00:00] [main/INFO]: x']);
    expect(filterLogLines(parsed, 'all', '').map((line) => line.lineNo)).toEqual([1, 2]);
  });
});
