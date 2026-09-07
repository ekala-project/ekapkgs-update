// ekapkgs_update.qnt — Formal specification of the ekapkgs-update pipeline
// Models the `run` command: checker service, updater service, 9-phase update
// workflow, concurrency constraints, database backoff, and hash discovery.

module ekapkgs_update {

  // ============================================================
  // Types
  // ============================================================

  // The 9 phases of the update workflow (in order)
  type UpdatePhase =
    | MetadataExtraction
    | SourceDiscovery
    | VersionSelection
    | SourceHashUpdate
    | DependencyHashUpdate
    | Build
    | Testing
    | PrCreation
    | Completed

  // Possible outcomes for a package update
  type UpdateOutcome =
    | Pending       // not yet started
    | InProgress    // currently being processed
    | Updated       // successfully completed all phases
    | Failed        // failed at some phase
    | Skipped       // skipped (e.g., backoff)

  // Commit strategy
  type CommitStrategy =
    | Worktrees
    | Branch

  // Hash discovery sub-states
  type HashState =
    | NoHash            // no hash operation needed or not started
    | InvalidWritten    // invalid hash written, awaiting build failure
    | CorrectParsed     // correct hash parsed from stderr
    | HashVerified      // rebuild succeeded with correct hash

  // A package in the system
  type Package = {
    id: int,
    phase: UpdatePhase,
    outcome: UpdateOutcome,
    hashState: HashState,
    failCount: int,       // number of consecutive failures (for backoff)
    backoffDays: int,     // current backoff: 2, 4, or 6
  }

  // ============================================================
  // Constants
  // ============================================================

  // System parameters
  pure val MAX_PACKAGES = 2
  pure val NUM_CPUS = 8
  pure val MAX_CHECKER_CONCURRENCY = 2    // max(4, concurrency * 2), reduced for verification
  pure val MAX_UPDATER_CONCURRENCY = 2    // max(1, cpus / 4)
  pure val BACKOFF_STEPS: int -> int = Map(0 -> 2, 1 -> 4, 2 -> 6)
  pure val MAX_BACKOFF = 6

  // All package IDs
  pure val PACKAGE_IDS = 1.to(MAX_PACKAGES)

  // Phase ordering for sequential progression
  pure def nextPhase(p: UpdatePhase): UpdatePhase = match p {
    | MetadataExtraction => SourceDiscovery
    | SourceDiscovery => VersionSelection
    | VersionSelection => SourceHashUpdate
    | SourceHashUpdate => DependencyHashUpdate
    | DependencyHashUpdate => Build
    | Build => Testing
    | Testing => PrCreation
    | PrCreation => Completed
    | Completed => Completed
  }

  // ============================================================
  // State Variables
  // ============================================================

  // The set of all packages
  var packages: int -> Package

  // Channel: set of package IDs that checker has sent to updater
  var channel: Set[int]

  // Checker state: set of package IDs currently being checked
  var checkerActive: Set[int]

  // Checker state: set of package IDs done checking (sent or skipped)
  var checkerDone: Set[int]

  // Updater state: set of package IDs currently being updated
  var updaterActive: Set[int]

  // Git mutex: is the git mutex held? (only relevant for Branch strategy)
  var gitMutexHeld: bool

  // Current commit strategy
  var commitStrategy: CommitStrategy

  // ============================================================
  // Helpers
  // ============================================================

  // Count packages in a given outcome
  pure def countByOutcome(pkgs: int -> Package, o: UpdateOutcome): int =
    PACKAGE_IDS.filter(id => pkgs.get(id).outcome == o).size()

  // Check if a package is in a hash-related phase
  pure def inHashPhase(pkg: Package): bool =
    pkg.phase == SourceHashUpdate or pkg.phase == DependencyHashUpdate

  // ============================================================
  // Initial State
  // ============================================================

  action init = all {
    packages' = PACKAGE_IDS.mapBy(id => {
      id: id,
      phase: MetadataExtraction,
      outcome: Pending,
      hashState: NoHash,
      failCount: 0,
      backoffDays: 2,
    }),
    channel' = Set(),
    checkerActive' = Set(),
    checkerDone' = Set(),
    updaterActive' = Set(),
    gitMutexHeld' = false,
    commitStrategy' = Worktrees,
  }

  // ============================================================
  // Checker Service Actions
  // ============================================================

  // Checker picks up a package for checking (respects semaphore)
  action checkerPickup(id: int): bool = all {
    // Package must be pending and not already being checked or done
    packages.get(id).outcome == Pending,
    not(checkerActive.contains(id)),
    not(checkerDone.contains(id)),
    // Semaphore: respect max checker concurrency
    checkerActive.size() < MAX_CHECKER_CONCURRENCY,
    // Update state
    checkerActive' = checkerActive.union(Set(id)),
    // Keep everything else the same
    packages' = packages,
    channel' = channel,
    checkerDone' = checkerDone,
    updaterActive' = updaterActive,
    gitMutexHeld' = gitMutexHeld,
    commitStrategy' = commitStrategy,
  }

  // Checker finishes checking and sends to updater via channel
  action checkerSend(id: int): bool = all {
    checkerActive.contains(id),
    // Package is not backed off (simplified: backoffDays must be <= 2 for first attempt)
    // Move to channel
    channel' = channel.union(Set(id)),
    checkerActive' = checkerActive.exclude(Set(id)),
    checkerDone' = checkerDone.union(Set(id)),
    // Keep everything else the same
    packages' = packages,
    updaterActive' = updaterActive,
    gitMutexHeld' = gitMutexHeld,
    commitStrategy' = commitStrategy,
  }

  // Checker skips a package (e.g., due to backoff)
  action checkerSkip(id: int): bool = all {
    checkerActive.contains(id),
    packages.get(id).outcome == Pending,
    // Mark as skipped
    packages' = packages.set(id, { ...packages.get(id), outcome: Skipped }),
    checkerActive' = checkerActive.exclude(Set(id)),
    checkerDone' = checkerDone.union(Set(id)),
    // Keep everything else the same
    channel' = channel,
    updaterActive' = updaterActive,
    gitMutexHeld' = gitMutexHeld,
    commitStrategy' = commitStrategy,
  }

  // ============================================================
  // Updater Service Actions
  // ============================================================

  // Updater picks up a package from the channel
  action updaterPickup(id: int): bool = all {
    channel.contains(id),
    // JoinSet bounded by concurrency
    updaterActive.size() < MAX_UPDATER_CONCURRENCY,
    // Remove from channel, add to active
    channel' = channel.exclude(Set(id)),
    updaterActive' = updaterActive.union(Set(id)),
    // Mark package as in progress
    packages' = packages.set(id, { ...packages.get(id), outcome: InProgress }),
    // Keep everything else the same
    checkerActive' = checkerActive,
    checkerDone' = checkerDone,
    gitMutexHeld' = gitMutexHeld,
    commitStrategy' = commitStrategy,
  }

  // ============================================================
  // Update Workflow Phase Transitions
  // ============================================================

  // Advance a package to the next phase (non-hash phases)
  action advancePhase(id: int): bool = all {
    updaterActive.contains(id),
    packages.get(id).outcome == InProgress,
    not(inHashPhase(packages.get(id))),
    packages.get(id).phase != Completed,
    // Cannot do PrCreation in Branch mode (no PR support)
    not(packages.get(id).phase == Testing and commitStrategy == Branch),
    val pkg = packages.get(id)
    val np = nextPhase(pkg.phase)
    packages' = packages.set(id, { ...pkg, phase: np }),
    // Keep everything else the same
    channel' = channel,
    checkerActive' = checkerActive,
    checkerDone' = checkerDone,
    updaterActive' = updaterActive,
    gitMutexHeld' = gitMutexHeld,
    commitStrategy' = commitStrategy,
  }

  // Skip PrCreation in Branch mode: go directly to Completed
  action skipPrInBranchMode(id: int): bool = all {
    updaterActive.contains(id),
    packages.get(id).outcome == InProgress,
    packages.get(id).phase == Testing,
    commitStrategy == Branch,
    val pkg = packages.get(id)
    packages' = packages.set(id, { ...pkg, phase: Completed }),
    channel' = channel,
    checkerActive' = checkerActive,
    checkerDone' = checkerDone,
    updaterActive' = updaterActive,
    gitMutexHeld' = gitMutexHeld,
    commitStrategy' = commitStrategy,
  }

  // ============================================================
  // Hash Discovery Workflow
  // ============================================================

  // Step 1: Write invalid hash
  action writeInvalidHash(id: int): bool = all {
    updaterActive.contains(id),
    packages.get(id).outcome == InProgress,
    inHashPhase(packages.get(id)),
    packages.get(id).hashState == NoHash,
    val pkg = packages.get(id)
    packages' = packages.set(id, { ...pkg, hashState: InvalidWritten }),
    channel' = channel,
    checkerActive' = checkerActive,
    checkerDone' = checkerDone,
    updaterActive' = updaterActive,
    gitMutexHeld' = gitMutexHeld,
    commitStrategy' = commitStrategy,
  }

  // Step 2: Build fails, parse correct hash from stderr
  action parseCorrectHash(id: int): bool = all {
    updaterActive.contains(id),
    packages.get(id).outcome == InProgress,
    inHashPhase(packages.get(id)),
    packages.get(id).hashState == InvalidWritten,
    val pkg = packages.get(id)
    packages' = packages.set(id, { ...pkg, hashState: CorrectParsed }),
    channel' = channel,
    checkerActive' = checkerActive,
    checkerDone' = checkerDone,
    updaterActive' = updaterActive,
    gitMutexHeld' = gitMutexHeld,
    commitStrategy' = commitStrategy,
  }

  // Step 3: Rebuild with correct hash succeeds
  action verifyHash(id: int): bool = all {
    updaterActive.contains(id),
    packages.get(id).outcome == InProgress,
    inHashPhase(packages.get(id)),
    packages.get(id).hashState == CorrectParsed,
    val pkg = packages.get(id)
    // Move to next phase with hash reset
    packages' = packages.set(id, { ...pkg, hashState: NoHash, phase: nextPhase(pkg.phase) }),
    channel' = channel,
    checkerActive' = checkerActive,
    checkerDone' = checkerDone,
    updaterActive' = updaterActive,
    gitMutexHeld' = gitMutexHeld,
    commitStrategy' = commitStrategy,
  }

  // ============================================================
  // Completion and Failure
  // ============================================================

  // Package completes successfully
  action completeUpdate(id: int): bool = all {
    updaterActive.contains(id),
    packages.get(id).outcome == InProgress,
    packages.get(id).phase == Completed,
    val pkg = packages.get(id)
    packages' = packages.set(id, { ...pkg, outcome: Updated, failCount: 0, backoffDays: 2 }),
    updaterActive' = updaterActive.exclude(Set(id)),
    channel' = channel,
    checkerActive' = checkerActive,
    checkerDone' = checkerDone,
    gitMutexHeld' = gitMutexHeld,
    commitStrategy' = commitStrategy,
  }

  // Package fails at current phase
  action failUpdate(id: int): bool = all {
    updaterActive.contains(id),
    packages.get(id).outcome == InProgress,
    packages.get(id).phase != Completed,
    val pkg = packages.get(id)
    val newFailCount = pkg.failCount + 1
    // Backoff: 2 -> 4 -> 6 (max)
    val newBackoff = if (newFailCount >= 3) MAX_BACKOFF
                     else if (newFailCount == 2) 4
                     else 2
    packages' = packages.set(id, {
      ...pkg,
      outcome: Failed,
      failCount: newFailCount,
      backoffDays: newBackoff,
      hashState: NoHash,
    }),
    updaterActive' = updaterActive.exclude(Set(id)),
    channel' = channel,
    checkerActive' = checkerActive,
    checkerDone' = checkerDone,
    gitMutexHeld' = gitMutexHeld,
    commitStrategy' = commitStrategy,
  }

  // ============================================================
  // Git Mutex (Branch Mode)
  // ============================================================

  // Acquire git mutex for PrCreation phase in Branch mode
  action acquireGitMutex(id: int): bool = all {
    commitStrategy == Branch,
    updaterActive.contains(id),
    packages.get(id).outcome == InProgress,
    packages.get(id).phase == PrCreation,
    not(gitMutexHeld),
    gitMutexHeld' = true,
    // Everything else unchanged
    packages' = packages,
    channel' = channel,
    checkerActive' = checkerActive,
    checkerDone' = checkerDone,
    updaterActive' = updaterActive,
    commitStrategy' = commitStrategy,
  }

  // Release git mutex
  action releaseGitMutex(id: int): bool = all {
    commitStrategy == Branch,
    updaterActive.contains(id),
    gitMutexHeld,
    gitMutexHeld' = false,
    packages' = packages,
    channel' = channel,
    checkerActive' = checkerActive,
    checkerDone' = checkerDone,
    updaterActive' = updaterActive,
    commitStrategy' = commitStrategy,
  }

  // ============================================================
  // System Step
  // ============================================================

  action step = any {
    nondet id = oneOf(PACKAGE_IDS)
    checkerPickup(id),

    nondet id = oneOf(PACKAGE_IDS)
    checkerSend(id),

    nondet id = oneOf(PACKAGE_IDS)
    checkerSkip(id),

    nondet id = oneOf(PACKAGE_IDS)
    updaterPickup(id),

    nondet id = oneOf(PACKAGE_IDS)
    advancePhase(id),

    nondet id = oneOf(PACKAGE_IDS)
    skipPrInBranchMode(id),

    nondet id = oneOf(PACKAGE_IDS)
    writeInvalidHash(id),

    nondet id = oneOf(PACKAGE_IDS)
    parseCorrectHash(id),

    nondet id = oneOf(PACKAGE_IDS)
    verifyHash(id),

    nondet id = oneOf(PACKAGE_IDS)
    completeUpdate(id),

    nondet id = oneOf(PACKAGE_IDS)
    failUpdate(id),
  }

  // ============================================================
  // Invariants
  // ============================================================

  // INV1: Checker concurrency never exceeds the semaphore limit
  val checkerConcurrencyBounded =
    checkerActive.size() <= MAX_CHECKER_CONCURRENCY

  // INV2: Updater concurrency never exceeds the JoinSet limit
  val updaterConcurrencyBounded =
    updaterActive.size() <= MAX_UPDATER_CONCURRENCY

  // INV3: A package cannot be in both checker and updater simultaneously
  val noCheckerUpdaterOverlap =
    checkerActive.intersect(updaterActive) == Set()

  // INV4: A package in the updater must be InProgress
  val updaterPackagesInProgress =
    updaterActive.forall(id => packages.get(id).outcome == InProgress)

  // INV5: Completed packages are not in any active set
  val completedNotActive =
    PACKAGE_IDS.forall(id =>
      (packages.get(id).outcome == Updated or packages.get(id).outcome == Failed or packages.get(id).outcome == Skipped)
        implies (not(checkerActive.contains(id)) and not(updaterActive.contains(id)) and not(channel.contains(id)))
    )

  // INV6: Backoff days are always 2, 4, or 6
  val backoffBounded =
    PACKAGE_IDS.forall(id => {
      val b = packages.get(id).backoffDays
      b == 2 or b == 4 or b == 6
    })

  // INV7: Hash discovery follows the correct sequence
  // If hashState is CorrectParsed, package must be in a hash phase
  // If hashState is InvalidWritten, package must be in a hash phase
  val hashStateConsistent =
    PACKAGE_IDS.forall(id => {
      val pkg = packages.get(id)
      (pkg.hashState != NoHash) implies (inHashPhase(pkg) or pkg.outcome == Failed or pkg.outcome == Pending)
    })

  // INV8: Channel only contains packages that are Pending (not yet picked up by updater)
  val channelPackagesPending =
    channel.forall(id => packages.get(id).outcome == Pending)

  // INV9: Phase ordering is monotonic — a package never goes backwards
  // (Encoded implicitly by nextPhase only going forward)

  // INV10: No package is in both the channel and the updater active set
  val channelUpdaterDisjoint =
    channel.intersect(updaterActive) == Set()

  // Combined invariant
  val safetyInvariant = and {
    checkerConcurrencyBounded,
    updaterConcurrencyBounded,
    noCheckerUpdaterOverlap,
    updaterPackagesInProgress,
    completedNotActive,
    backoffBounded,
    hashStateConsistent,
    channelPackagesPending,
    channelUpdaterDisjoint,
  }

  // ============================================================
  // Temporal Properties
  // ============================================================

  // Every package eventually leaves the Pending state
  // (liveness — not checked by verify, but good documentation)
  temporal eventualProgress =
    PACKAGE_IDS.forall(id =>
      eventually(packages.get(id).outcome != Pending)
    )

  // ============================================================
  // Helper: no-op action for assertion-only steps
  // ============================================================

  action unchanged = all {
    packages' = packages,
    channel' = channel,
    checkerActive' = checkerActive,
    checkerDone' = checkerDone,
    updaterActive' = updaterActive,
    gitMutexHeld' = gitMutexHeld,
    commitStrategy' = commitStrategy,
  }

  // ============================================================
  // Tests
  // ============================================================

  // Test: initial state satisfies invariants
  run initSatisfiesInvariant = {
    init.then(all {
      assert(safetyInvariant),
      assert(PACKAGE_IDS.forall(id => packages.get(id).outcome == Pending)),
      assert(channel == Set()),
      assert(checkerActive == Set()),
      assert(updaterActive == Set()),
      unchanged,
    })
  }

  // Test: checker picks up and sends a package
  run checkerFlow = {
    init
      .then(checkerPickup(1))
      .then(all {
        assert(checkerActive.contains(1)),
        assert(checkerActive.size() == 1),
        checkerSend(1),
      })
      .then(all {
        assert(channel.contains(1)),
        assert(not(checkerActive.contains(1))),
        assert(checkerDone.contains(1)),
        assert(safetyInvariant),
        unchanged,
      })
  }

  // Test: full happy path for one package
  run happyPath = {
    init
      .then(checkerPickup(1))
      .then(checkerSend(1))
      .then(updaterPickup(1))
      .then(all {
        assert(packages.get(1).outcome == InProgress),
        assert(packages.get(1).phase == MetadataExtraction),
        advancePhase(1),
      })
      .then(advancePhase(1))     // SourceDiscovery -> VersionSelection
      .then(advancePhase(1))     // VersionSelection -> SourceHashUpdate
      // Hash discovery for source hash
      .then(writeInvalidHash(1))
      .then(parseCorrectHash(1))
      .then(verifyHash(1))       // SourceHashUpdate -> DependencyHashUpdate
      // Hash discovery for dependency hash
      .then(writeInvalidHash(1))
      .then(parseCorrectHash(1))
      .then(verifyHash(1))       // DependencyHashUpdate -> Build
      .then(advancePhase(1))     // Build -> Testing
      .then(advancePhase(1))     // Testing -> PrCreation
      .then(advancePhase(1))     // PrCreation -> Completed
      .then(all {
        assert(packages.get(1).phase == Completed),
        completeUpdate(1),
      })
      .then(all {
        assert(packages.get(1).outcome == Updated),
        assert(not(updaterActive.contains(1))),
        assert(safetyInvariant),
        unchanged,
      })
  }

  // Test: failure path with backoff stepping
  run failureWithBackoff = {
    init
      .then(checkerPickup(1))
      .then(checkerSend(1))
      .then(updaterPickup(1))
      .then(advancePhase(1))     // MetadataExtraction -> SourceDiscovery
      .then(failUpdate(1))       // Fail at SourceDiscovery
      .then(all {
        assert(packages.get(1).outcome == Failed),
        assert(packages.get(1).failCount == 1),
        assert(packages.get(1).backoffDays == 2),
        assert(safetyInvariant),
        unchanged,
      })
  }

  // Test: checker respects semaphore
  run checkerSemaphore = {
    init
      .then(checkerPickup(1))
      .then(checkerPickup(2))
      .then(all {
        assert(checkerActive.size() == 2),
        assert(checkerActive.size() <= MAX_CHECKER_CONCURRENCY),
        assert(safetyInvariant),
        unchanged,
      })
  }

  // Test: updater respects concurrency limit
  run updaterConcurrency = {
    init
      .then(checkerPickup(1))
      .then(checkerSend(1))
      .then(checkerPickup(2))
      .then(checkerSend(2))
      .then(updaterPickup(1))
      .then(updaterPickup(2))
      .then(all {
        assert(updaterActive.size() == 2),
        assert(updaterActive.size() <= MAX_UPDATER_CONCURRENCY),
        assert(safetyInvariant),
        unchanged,
      })
  }

  // Test: skip package
  run skipPath = {
    init
      .then(checkerPickup(1))
      .then(checkerSkip(1))
      .then(all {
        assert(packages.get(1).outcome == Skipped),
        assert(not(checkerActive.contains(1))),
        assert(checkerDone.contains(1)),
        assert(safetyInvariant),
        unchanged,
      })
  }
}
