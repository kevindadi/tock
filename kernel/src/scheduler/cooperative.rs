//! Tock 的协作调度器
//!
//! 此调度程序以循环方式运行所有进程，但不使用调度程序计时器来强制执行进程时间片。
//! 也就是说，所有进程都是协同运行的。
//! 进程一直运行直到它们产生或停止执行（即它们崩溃或退出）。
//!
//! 当用户空间进程正在执行时发生硬件中断时，此调度程序执行中断的上半部分，
//! 然后立即停止执行用户空间进程并处理中断的下半部分。
//! 但是，它会继续执行之前正在执行的用户空间进程,简单来说上半部分不可抢占
use crate::collections::list::{List, ListLink, ListNode};
use crate::kernel::{Kernel, StoppedExecutingReason};
use crate::platform::chip::Chip;
use crate::process::Process;
use crate::scheduler::{Scheduler, SchedulingDecision};

/// 调度程序用于跟踪进程的链表中的节点
pub struct CoopProcessNode<'a> {
    proc: &'static Option<&'static dyn Process>,
    next: ListLink<'a, CoopProcessNode<'a>>,
}

impl<'a> CoopProcessNode<'a> {
    pub fn new(proc: &'static Option<&'static dyn Process>) -> CoopProcessNode<'a> {
        CoopProcessNode {
            proc,
            next: ListLink::empty(),
        }
    }
}

impl<'a> ListNode<'a, CoopProcessNode<'a>> for CoopProcessNode<'a> {
    fn next(&'a self) -> &'a ListLink<'a, CoopProcessNode> {
        &self.next
    }
}

/// 协作调度器
pub struct CooperativeSched<'a> {
    pub processes: List<'a, CoopProcessNode<'a>>,
}

impl<'a> CooperativeSched<'a> {
    pub const fn new() -> CooperativeSched<'a> {
        CooperativeSched {
            processes: List::new(),
        }
    }
}

impl<'a, C: Chip> Scheduler<C> for CooperativeSched<'a> {
    fn next(&self, kernel: &Kernel) -> SchedulingDecision {
        if kernel.processes_blocked() {
            // No processes ready
            SchedulingDecision::TrySleep
        } else {
            let mut next = None;
            // 这将被替换，如果 processes_blocked() 为假，则保证进程准备就绪

            // Find next ready process. Place any *empty* process slots, or not-ready
            // processes, at the back of the queue.
            for node in self.processes.iter() {
                match node.proc {
                    Some(proc) => {
                        if proc.ready() {
                            next = Some(proc.processid());
                            break;
                        }
                        self.processes.push_tail(self.processes.pop_head().unwrap());
                    }
                    None => {
                        self.processes.push_tail(self.processes.pop_head().unwrap());
                    }
                }
            }

            SchedulingDecision::RunProcess((next.unwrap(), None))
        }
    }

    fn result(&self, result: StoppedExecutingReason, _: Option<u32>) {
        let reschedule = match result {
            StoppedExecutingReason::KernelPreemption => true,
            _ => false,
        };
        if !reschedule {
            self.processes.push_tail(self.processes.pop_head().unwrap());
        }
    }
}
