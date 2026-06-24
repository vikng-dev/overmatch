# The camera is the aiming device, so zoom never moves the aim

The third-person camera doubles as the gun sight — `aim` casts a ray through screen center — so the camera's look direction must stay the player's, and **zoom only changes the orbit distance**. Distance slides the camera along its own view axis, which provably cannot move the aim point (the screen-center ray is unchanged; only the eye slides along it).

We evaluated an authored "rail" camera that varies height/angle with the zoom level. It's incompatible with aiming for two reasons: moving the camera *off* the view axis shifts where the screen-center ray lands (parallax → the aim drifts as you zoom), and authoring the look *angle* seizes the very degree of freedom aiming needs. So the rig is a free-aim orbit, full stop.

A cinematic or commander's-hatch view can still exist later, but only as a **separate, non-aiming camera mode** — and a true aim-holding cinematic rail would require inverting control so the *aim point* is primary and the camera follows it (the DesiredAim inversion). Recorded so the rejected rail isn't re-proposed.
