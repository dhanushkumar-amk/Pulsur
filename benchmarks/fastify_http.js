const fastify = require('fastify')({ logger: false });

fastify.get('/', async (request, reply) => {
    return { 
        message: "Fastify Baseline Online", 
        version: process.version 
    };
});

const start = async () => {
    try {
        await fastify.listen({ port: 3002 });
        console.log(`Fastify Baseline: http://localhost:3002`);
    } catch (err) {
        process.exit(1);
    }
}
start();
