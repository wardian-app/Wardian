---
schema: 2
id: minimal
name: Minimal
nodes:
  - id: trigger-1
    type: manual_trigger
  - id: plan
    type: task
    fields:
      agent: role:planner
      prompt: Plan the work
edges:
  - from: trigger-1
    to: plan
---

# Minimal

A tiny workflow used in tests.
