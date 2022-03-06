//! Tock 的循环调度程序
//!
//! 这个调度器特别是一个带有中断的循环调度器。
//!
//! See: <https://www.eecs.umich.edu/courses/eecs461/lecture/SWArchitecture.pdf>
//! for details.
//!
//! 当用户空间进程正在执行时发生硬件中断时，此调度程序执行中断的上半部分，
//! 然后立即停止执行用户空间进程并处理中断的下半部分。
//! 这个设计决定是为了模仿原始 Tock 调度程序的行为。
//! 为了确保时间片的公平使用，当用户空间进程被中断时，调度程序计时器会暂停，
//! 并且相同的进程会使用与中断时相同的调度程序计时器值来恢复。

use core::cell::Cell;

use crate::collections::list::{List, ListLink, ListNode};
use crate::kernel::{Kernel, StoppedExecutingReason};
use crate::platform::chip::Chip;
use crate::process::Process;
use crate::scheduler::{Scheduler, SchedulingDecision};

/// 调度程序用来跟踪进程的链表中的一个节点。
/// 每个节点都有一个指针，指向进程数组中的一个slot
pub struct RoundRobinProcessNode<'a> {
    proc: &'static Option<&'static dyn Process>,
    next: ListLink<'a, RoundRobinProcessNode<'a>>,
}

impl<'a> RoundRobinProcessNode<'a> {
    pub fn new(proc: &'static Option<&'static dyn Process>) -> RoundRobinProcessNode<'a> {
        RoundRobinProcessNode {
            proc,
            next: ListLink::empty(),
        }
    }
}

impl<'a> ListNode<'a, RoundRobinProcessNode<'a>> for RoundRobinProcessNode<'a> {
    fn next(&'a self) -> &'a ListLink<'a, RoundRobinProcessNode> {
        &self.next
    }
}

/// Round Robin Scheduler
pub struct RoundRobinSched<'a> {
    time_remaining: Cell<u32>,
    pub processes: List<'a, RoundRobinProcessNode<'a>>,
    last_rescheduled: Cell<bool>,
}

impl<'a> RoundRobinSched<'a> {
    /// 进程在被抢占之前可以运行多长时间
    const DEFAULT_TIMESLICE_US: u32 = 10000;
    pub const fn new() -> RoundRobinSched<'a> {
        RoundRobinSched {
            time_remaining: Cell::new(Self::DEFAULT_TIMESLICE_US),
            processes: List::new(),
            last_rescheduled: Cell::new(false),
        }
    }
}

impl<'a, C: Chip> Scheduler<C> for RoundRobinSched<'a> {
    fn next(&self, kernel: &Kernel) -> SchedulingDecision {
        if kernel.processes_blocked() {
            // No processes ready
            SchedulingDecision::TrySleep
        } else {
            let mut next = None;
            // This will be replaced, bc a process is guaranteed
            // to be ready if processes_blocked() is false

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
            let timeslice = if self.last_rescheduled.get() {
                self.time_remaining.get()
            } else {
                // grant a fresh timeslice
                self.time_remaining.set(Self::DEFAULT_TIMESLICE_US);
                Self::DEFAULT_TIMESLICE_US
            };
            assert!(timeslice != 0);

            SchedulingDecision::RunProcess((next.unwrap(), Some(timeslice)))
        }
    }

    fn result(&self, result: StoppedExecutingReason, execution_time_us: Option<u32>) {
        let execution_time_us = execution_time_us.unwrap(); // should never fail
        let reschedule = match result {
            StoppedExecutingReason::KernelPreemption => {
                if self.time_remaining.get() > execution_time_us {
                    self.time_remaining
                        .set(self.time_remaining.get() - execution_time_us);
                    true
                } else {
                    false
                }
            }
            _ => false,
        };
        self.last_rescheduled.set(reschedule);
        if !reschedule {
            self.processes.push_tail(self.processes.pop_head().unwrap());
        }
    }
}
