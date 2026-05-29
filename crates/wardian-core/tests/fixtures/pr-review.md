---
schema: 2
id: pr-review
name: PR Review
nodes:
  - id: trigger-1
    type: manual_trigger
  - id: plan
    type: task
    fields:
      agent: role:planner
      prompt: Draft a plan for the change
  - id: loop-1
    type: loop
    fields:
      max_iterations: 3
  - id: implement
    type: task
    parent: loop-1
    fields:
      agent: role:coder
      prompt: Apply the plan
  - id: test
    type: shell
    parent: loop-1
    fields:
      command: cargo test
  - id: ship
    type: task
    fields:
      agent: role:coder
      prompt: Open the PR
edges:
  - from: trigger-1
    to: plan
  - from: plan
    to: loop-1
  - from: loop-1
    to: implement
    from_port: body
  - from: implement
    to: test
  - from: loop-1
    to: ship
    from_port: done
---

# PR Review

Plan, then iterate implement/test inside a loop, then ship.
