#include <unistd.h>
#include <sys/wait.h>

int main(int argc, char **argv) {
    if (argc > 1) {
        pid_t child = fork();
        if (child < 0) return 1;
        if (child == 0) {
            execvp(argv[1], argv + 1);
            _exit(127);
        }
        int status = 0;
        if (waitpid(child, &status, 0) < 0 || !WIFEXITED(status) || WEXITSTATUS(status) != 0) return 1;
    }
    sleep(300);
    return 0;
}
