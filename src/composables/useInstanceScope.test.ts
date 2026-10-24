import { describe, it, expect } from 'vitest';
import { ref } from 'vue';
import { useInstanceScope } from './useInstanceScope';

describe('useInstanceScope', () => {
  it('未切换实例时任务保持有效', () => {
    const name = ref('A');
    const scope = useInstanceScope(() => name.value);
    const task = scope.begin();

    expect(task.instanceName).toBe('A');
    expect(scope.isCurrent(task)).toBe(true);
  });

  it('切换实例后旧任务失效，新任务有效', () => {
    const name = ref('A');
    const scope = useInstanceScope(() => name.value);
    const oldTask = scope.begin();

    name.value = 'B';

    const newTask = scope.begin();
    expect(scope.isCurrent(oldTask)).toBe(false);
    expect(newTask.instanceName).toBe('B');
    expect(scope.isCurrent(newTask)).toBe(true);
  });

  it('切回原实例不能复活旧任务', () => {
    const name = ref('A');
    const scope = useInstanceScope(() => name.value);
    const task = scope.begin();

    name.value = 'B';
    name.value = 'A';

    expect(scope.isCurrent(task)).toBe(false);
  });
});
