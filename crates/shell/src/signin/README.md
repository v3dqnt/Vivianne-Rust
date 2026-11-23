# shell/src/signin

Signing in to a coding agent from inside the app. A small window runs the agent's own sign-in command in a real
terminal (the same terminal the rail uses), and while it is open the app keeps checking whether the agent is signed
in. When it is, the window says so, calls back, and closes. Nothing here asks for or keeps a password: the agent's
command does the signing in.
