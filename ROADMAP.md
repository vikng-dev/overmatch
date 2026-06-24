# Deliberately lean roadmap - ideas always shift, this is a general direction

Phase 0: Model rigging and control basics

Phase 1: Single player tank moving and shooting in a world

Phase 2: Action/control mapping - lift raw device reads into an intent layer (devices -> Controls -> gameplay), so rebinding/gamepad and server-authoritative netcode hang off one seam. Hand-rolled first; reach for leafwing-input-manager only when it strains.

Phase 3: Ballistics and armor pen simulation

Phase 4: Composition proof - multiple tank variants

Phase 5: world composition: levels/maps, garage, UI

Phase 6: Multiplayer