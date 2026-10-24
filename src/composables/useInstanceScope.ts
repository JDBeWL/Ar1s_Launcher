/**
 * 实例切换守卫
 * 异步任务开始时取快照，切换到别的实例后旧任务不得再回填状态或继续批量写入。
 */
import { watch } from 'vue';

export interface InstanceTask {
  readonly instanceName: string;
  readonly token: number;
}

export function useInstanceScope(getName: () => string) {
  let token = 0;

  // sync 刷新：A→B→A 这种同一 tick 内来回切换也要让旧任务立刻失效
  watch(
    getName,
    () => {
      token += 1;
    },
    { flush: 'sync' },
  );

  function begin(): InstanceTask {
    return { instanceName: getName(), token };
  }

  function isCurrent(task: InstanceTask): boolean {
    return task.token === token && task.instanceName === getName();
  }

  return { begin, isCurrent };
}
